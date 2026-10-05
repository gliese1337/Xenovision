//! Photoreceptor-to-perceptual-channel transform (design doc §2.2):
//! receptor activation, chromatic adaptation (via `adaptation`), and the
//! luminance + opponent (chroma) construction, combined into a per-
//! species+illuminant `Pipeline` that's cheap to evaluate per stimulus.

use crate::adaptation::{derive_adaptation_matrix, AdaptationResult};
use crate::curve::SpectralCurve;
use crate::curve_set::CurveSet;
use crate::linalg::{self, dot, norm};

/// Raw receptor activation `Q = ∫ S(λ)·I(λ) dλ` (§2.2.2), via trapezoidal
/// integration over the union of `a` and `b`'s domains, treating each
/// curve as zero outside its own domain.
///
/// Note: treating a curve as exactly zero just past its last point creates
/// a hard discontinuity there if the curve's value doesn't already
/// taper to ~zero by that point. Trapezoidal integration smooths across
/// that discontinuity (half of one `step_nm`-wide "phantom" triangle per
/// edge), which is negligible for ordinary sensitivity/illuminant curves
/// (they taper off before truncation) but matters for an artificially
/// sharp-edged test curve - see the box-function test below, which uses a
/// small `step_nm` specifically to keep that bias below its tolerance.
pub fn integrate_product(a: &SpectralCurve, b: &SpectralCurve, step_nm: f64) -> f64 {
    let (lo, hi) = match (a.domain(), b.domain()) {
        (Some((a0, a1)), Some((b0, b1))) => (a0.min(b0), a1.max(b1)),
        (Some(d), None) | (None, Some(d)) => d,
        (None, None) => return 0.0,
    };
    if hi <= lo {
        return 0.0;
    }
    let ia = a.interpolant();
    let ib = b.interpolant();
    let f = |wl: f64| ia.value_at(wl).unwrap_or(0.0) * ib.value_at(wl).unwrap_or(0.0);

    let n = ((hi - lo) / step_nm).ceil().max(1.0) as usize;
    let dx = (hi - lo) / n as f64;
    let mut sum = 0.0;
    let mut prev = f(lo);
    for i in 1..=n {
        let wl = lo + i as f64 * dx;
        let cur = f(wl);
        sum += 0.5 * (prev + cur) * dx;
        prev = cur;
    }
    sum
}

/// Raw (pre-adaptation) activations `Q_i = ∫ S_i(λ)·stimulus(λ) dλ`
/// (§2.2.2 Step 1) for every curve in `curves` against `stimulus` - the
/// same formula `Pipeline` already uses internally for its colorspace
/// receptor curves before adaptation, exposed generically here so it
/// applies identically to curves that never enter the opponent-process
/// pipeline at all (§10.1: curves held in a `CurveSet`'s
/// `isolated_curves`, e.g. the swallowtail butterfly's violet/broad-
/// band receptors or - the all-isolated-curves case - a Mantis Shrimp
/// fixture's entire receptor set, §10.2). There's no "adapted" version
/// of this for isolated curves to show instead: adaptation is itself
/// a property of the colorspace curves' chromatic-adaptation matrix,
/// which an isolated curve was never part of deriving.
pub fn raw_activations(
    curves: &[SpectralCurve],
    stimulus: &SpectralCurve,
    step_nm: f64,
) -> Vec<f64> {
    curves
        .iter()
        .map(|c| c.saturate(integrate_product(c, stimulus, step_nm)))
        .collect()
}

/// The generic per-receptor candidate opponent contrast (§2.2.4): for
/// receptor `i`, `Cand_i = Q'_i - mean(Q'_j for j != i)`, as coefficient
/// rows ready for `build_chroma_axes`. For `n == 1` returns a single row
/// coinciding with the (implicit) luminance direction, so that
/// orthogonalizing it against luminance correctly yields zero chroma
/// channels (§7.2's monochromat degenerate case).
pub fn generic_candidate_rows(n: usize) -> Vec<Vec<f64>> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![vec![1.0]];
    }
    (0..n)
        .map(|i| {
            let mut row = vec![-1.0 / (n as f64 - 1.0); n];
            row[i] = 1.0;
            row
        })
        .collect()
}

/// Projects `v` to be orthogonal to every vector in `bases` (classical
/// Gram-Schmidt), returning `None` if what's left has negligible norm
/// (i.e. `v` was linearly dependent on `bases`).
fn orthogonalize_against(bases: &[Vec<f64>], v: &[f64], eps: f64) -> Option<Vec<f64>> {
    let mut w = v.to_vec();
    for b in bases {
        let bb = dot(b, b);
        if bb < eps {
            continue;
        }
        let proj = dot(&w, b) / bb;
        for (wi, bi) in w.iter_mut().zip(b.iter()) {
            *wi -= proj * bi;
        }
    }
    let n = norm(&w);
    if n > eps {
        Some(w.iter().map(|x| x / n).collect())
    } else {
        None
    }
}

/// Builds the `N-1` chroma axes (§2.2.4 step 2) from `candidates`
/// (`N` generic candidates, or however many literature-defined contrasts a
/// species provides), orthogonalized against `luminance_weights` and
/// against each other. Generically drops exactly one candidate per unit
/// of linear dependency on what came before - e.g. `N` generic candidates
/// in the `(N-1)`-dimensional orthogonal complement of luminance reduce to
/// `N-1` axes; `N-1` already-independent literature contrasts (e.g.
/// human's L-M, S-(L+M)) pass through unchanged.
pub fn build_chroma_axes(
    candidates: &[Vec<f64>],
    luminance_weights: &[f64],
    eps: f64,
) -> Vec<Vec<f64>> {
    let mut bases = vec![luminance_weights.to_vec()];
    let mut chroma = Vec::new();
    for c in candidates {
        if let Some(w) = orthogonalize_against(&bases, c, eps) {
            bases.push(w.clone());
            chroma.push(w);
        }
    }
    chroma
}

#[derive(Debug, Clone)]
pub struct Coordinates {
    pub luminance: f64,
    pub chroma: Vec<f64>,
    pub saturation: f64,
    /// Adapted receptor activations `Q'` - not part of the design doc's
    /// §3.2 output vector, but needed alongside it for the ΔS metric
    /// (§3.3.2), which operates on `Q'` directly rather than on
    /// luminance/chroma.
    pub adapted_activations: Vec<f64>,
}

impl Coordinates {
    /// `chroma`'s hyperspherical hue angles - `chroma.len() - 1` of them
    /// (so `N - 2` for an `N`-receptor species, chroma having `N - 1`
    /// dimensions), with `saturation` as the corresponding radius. Empty
    /// for a monochromat or dichromat (0 or 1 chroma dimensions) - a
    /// single signed chroma value has no angle to decompose, just the
    /// sign it already carries.
    pub fn hue_angles(&self) -> Vec<f64> {
        linalg::hyperspherical_angles(&self.chroma)
    }
}

/// A fully-built per-species, per-reference-illuminant pipeline (§2.2's
/// combined transform): the expensive part (deriving the adaptation
/// matrix, §2.2.3) is done once in `build`, so `coordinates` is cheap to
/// call per stimulus.
pub struct Pipeline {
    colorspace_curves: Vec<SpectralCurve>,
    /// Carried alongside `colorspace_curves` so individual photoreceptor
    /// activations (§10.1) can be displayed for curves that don't
    /// participate in the opponent-process pipeline at all (e.g. the
    /// swallowtail butterfly's violet/broad-band receptors, §2.3.9) -
    /// consistent with "the data model carries everything the pipeline
    /// needs" rather than requiring a caller to separately hold onto the
    /// originating `CurveSet` just for this.
    isolated_curves: Vec<SpectralCurve>,
    adaptation: AdaptationResult,
    q_env: Vec<f64>,
    luminance_weights: Vec<f64>,
    chroma_axes: Vec<Vec<f64>>,
    /// Vorobyev-Osorio per-receptor noise terms (§3.3.2), if `species`
    /// had complete omega+eta data on every curve - see
    /// `CurveSet::receptor_noise`.
    noise: Option<Vec<f64>>,
    step_nm: f64,
}

impl Pipeline {
    /// Builds the pipeline from a species' `CurveSet` directly: luminance
    /// weights, opponent-contrast candidate rows, and ΔS noise terms all
    /// come from the set's own data (§4's representability goal - the
    /// data model carries everything the pipeline needs, with no
    /// separately-maintained parameter vectors that could drift out of
    /// sync with it).
    pub fn build(species: CurveSet, reference_illuminant: &SpectralCurve, step_nm: f64) -> Self {
        let luminance_weights = species.luminance_weights(step_nm);
        let candidate_rows = species.candidate_rows();
        let noise = species.receptor_noise();
        let colorspace_curves = species.colorspace_curves;
        let isolated_curves = species.isolated_curves;

        let adaptation = derive_adaptation_matrix(&colorspace_curves, step_nm);
        let q_env: Vec<f64> = raw_activations(&colorspace_curves, reference_illuminant, step_nm);
        let chroma_axes = build_chroma_axes(&candidate_rows, &luminance_weights, 1e-9);

        Pipeline {
            colorspace_curves,
            isolated_curves,
            adaptation,
            q_env,
            luminance_weights,
            chroma_axes,
            noise,
            step_nm,
        }
    }

    pub fn adaptation_result(&self) -> &AdaptationResult {
        &self.adaptation
    }

    pub fn chroma_axis_count(&self) -> usize {
        self.chroma_axes.len()
    }

    pub fn noise(&self) -> Option<&[f64]> {
        self.noise.as_deref()
    }

    /// Adapted receptor activations `Q'` for raw activations `q`, via the
    /// von Kries-style diagonal scaling in adaptation space (§2.2.3 step
    /// 2): transform into adaptation space, scale each channel by the
    /// inverse of its response to the reference illuminant, transform
    /// back.
    fn adapt(&self, q: &[f64]) -> Vec<f64> {
        let aq_env = self.adaptation.matrix.mul_vec(&self.q_env);
        let aq = self.adaptation.matrix.mul_vec(q);
        let scaled: Vec<f64> = aq
            .iter()
            .zip(aq_env.iter())
            .map(|(&v, &e)| if e.abs() < 1e-12 { v } else { v / e })
            .collect();
        self.adaptation.inverse.mul_vec(&scaled)
    }

    /// Adapted receptor activations `Q'` for a stimulus spectrum - the
    /// same quantity `coordinates` derives luminance/chroma from, exposed
    /// directly for the ΔS metric (§3.3.2).
    pub fn adapted_activations(&self, stimulus: &SpectralCurve) -> Vec<f64> {
        self.adapt(&self.colorspace_activations(stimulus))
    }

    /// Raw (pre-adaptation) activations for the colorspace receptor
    /// curves (§10.1) - the same `Q` `adapted_activations` adapts,
    /// exposed on its own so a UI can display both the raw and adapted
    /// value per receptor rather than only the adapted one.
    pub fn colorspace_activations(&self, stimulus: &SpectralCurve) -> Vec<f64> {
        raw_activations(&self.colorspace_curves, stimulus, self.step_nm)
    }

    /// Raw activations for the isolated curves (§10.1, §10.2) - never
    /// adapted, since adaptation is a property of the colorspace
    /// curves' own chromatic-adaptation matrix, which these curves were
    /// never part of deriving.
    pub fn isolated_activations(&self, stimulus: &SpectralCurve) -> Vec<f64> {
        raw_activations(&self.isolated_curves, stimulus, self.step_nm)
    }

    /// Display names for the colorspace receptor curves, in the same
    /// order `colorspace_activations`/`adapted_activations`/
    /// `coordinates` use.
    pub fn colorspace_curve_names(&self) -> Vec<&str> {
        self.colorspace_curves
            .iter()
            .map(|c| c.name.as_str())
            .collect()
    }

    /// Display names for the isolated curves, in the same order
    /// `isolated_activations` uses.
    pub fn isolated_curve_names(&self) -> Vec<&str> {
        self.isolated_curves
            .iter()
            .map(|c| c.name.as_str())
            .collect()
    }

    /// The perceptual coordinates `[L, C_1, ..., C_(N-1)]` plus saturation
    /// (§2.2.5) for a stimulus spectrum.
    pub fn coordinates(&self, stimulus: &SpectralCurve) -> Coordinates {
        let q_adapted = self.adapted_activations(stimulus);
        let luminance = dot(&self.luminance_weights, &q_adapted);
        let chroma: Vec<f64> = self
            .chroma_axes
            .iter()
            .map(|row| dot(row, &q_adapted))
            .collect();
        let saturation = chroma.iter().map(|c| c * c).sum::<f64>().sqrt();
        Coordinates {
            luminance,
            chroma,
            saturation,
            adapted_activations: q_adapted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::CurveType;
    use crate::fixtures;

    fn flat_curve(name: &str, value: f64) -> SpectralCurve {
        SpectralCurve::new(name, CurveType::Illumination)
            .with_points(vec![(300.0, value), (750.0, value)])
    }

    // --- integrate_product: hand-calculable cases ---

    #[test]
    fn integrate_product_of_two_constant_overlapping_boxes() {
        // S(λ)=2 on [100,200], I(λ)=3 on [150,250]. Overlap is [150,200],
        // length 50, product value 6 there (0 elsewhere) -> integral = 300.
        let s = SpectralCurve::new("s", CurveType::Sensitivity)
            .with_points(vec![(100.0, 2.0), (200.0, 2.0)]);
        let i = SpectralCurve::new("i", CurveType::Illumination)
            .with_points(vec![(150.0, 3.0), (250.0, 3.0)]);
        // A fine step keeps the (expected, see integrate_product's doc
        // comment) domain-edge discretization bias well below tolerance;
        // ordinary curves taper to ~zero before truncation so don't need this.
        let q = integrate_product(&s, &i, 0.01);
        assert!((q - 300.0).abs() < 0.1, "q={q}");
    }

    #[test]
    fn integrate_product_of_disjoint_curves_is_zero() {
        let s = SpectralCurve::new("s", CurveType::Sensitivity)
            .with_points(vec![(100.0, 1.0), (200.0, 1.0)]);
        let i = SpectralCurve::new("i", CurveType::Illumination)
            .with_points(vec![(300.0, 1.0), (400.0, 1.0)]);
        assert_eq!(integrate_product(&s, &i, 1.0), 0.0);
    }

    // --- generic candidate rows + chroma axis construction ---

    #[test]
    fn n1_monochromat_has_zero_chroma_axes() {
        let candidates = generic_candidate_rows(1);
        let luminance = vec![1.0];
        let chroma = build_chroma_axes(&candidates, &luminance, 1e-9);
        assert_eq!(chroma.len(), 0, "monochromat must have no chroma channels");
    }

    #[test]
    fn generic_n_candidates_reduce_to_n_minus_1_chroma_axes() {
        for n in 2..=5 {
            let candidates = generic_candidate_rows(n);
            let luminance = vec![1.0 / n as f64; n];
            let chroma = build_chroma_axes(&candidates, &luminance, 1e-9);
            assert_eq!(chroma.len(), n - 1, "n={n}");
            // Every chroma axis must be orthogonal to luminance.
            for axis in &chroma {
                assert!(dot(axis, &luminance).abs() < 1e-9, "n={n} axis={axis:?}");
            }
            // And mutually orthogonal.
            for i in 0..chroma.len() {
                for j in (i + 1)..chroma.len() {
                    assert!(
                        dot(&chroma[i], &chroma[j]).abs() < 1e-9,
                        "n={n} i={i} j={j}"
                    );
                }
            }
        }
    }

    #[test]
    fn human_literature_contrasts_orthogonalize_to_exactly_2_chroma_axes() {
        let human = fixtures::human();
        let luminance = human.luminance_weights(1.0);
        let candidates = human.candidate_rows();
        let chroma = build_chroma_axes(&candidates, &luminance, 1e-9);
        assert_eq!(
            chroma.len(),
            2,
            "human (N=3) must yield exactly 2 chroma axes"
        );
        for axis in &chroma {
            assert!(
                dot(axis, &luminance).abs() < 1e-9,
                "axis not orthogonal to luminance: {axis:?}"
            );
        }
        assert!(
            dot(&chroma[0], &chroma[1]).abs() < 1e-9,
            "chroma axes not mutually orthogonal"
        );
    }

    // --- end-to-end Pipeline, human fixture ---

    #[test]
    fn human_pipeline_has_exactly_2_chroma_channels() {
        let illuminant = flat_curve("flat", 1.0);
        let pipeline = Pipeline::build(fixtures::human(), &illuminant, 1.0);
        assert_eq!(pipeline.chroma_axis_count(), 2);
    }

    #[test]
    fn human_pipeline_coordinates_are_finite_and_saturation_matches_chroma() {
        let illuminant = flat_curve("flat", 1.0);
        let pipeline = Pipeline::build(fixtures::human(), &illuminant, 1.0);
        // A reddish stimulus: flat background with a bump near 600nm.
        let stimulus = SpectralCurve::new("reddish", CurveType::Reflectance).with_points(vec![
            (300.0, 0.2),
            (500.0, 0.2),
            (600.0, 0.9),
            (700.0, 0.9),
            (750.0, 0.9),
        ]);
        let coords = pipeline.coordinates(&stimulus);
        assert!(coords.luminance.is_finite());
        assert_eq!(coords.chroma.len(), 2);
        assert!(coords.chroma.iter().all(|c| c.is_finite()));
        let expected_sat = (coords.chroma[0].powi(2) + coords.chroma[1].powi(2)).sqrt();
        assert!((coords.saturation - expected_sat).abs() < 1e-9);
    }

    #[test]
    fn human_pipeline_hue_angles_match_hand_calculation() {
        // N=3 -> 2 chroma dims -> exactly 1 hue angle, the plain 2D
        // polar angle of the chroma vector.
        let illuminant = flat_curve("flat", 1.0);
        let pipeline = Pipeline::build(fixtures::human(), &illuminant, 1.0);
        let stimulus = SpectralCurve::new("reddish", CurveType::Reflectance).with_points(vec![
            (300.0, 0.2),
            (600.0, 0.9),
            (750.0, 0.9),
        ]);
        let coords = pipeline.coordinates(&stimulus);
        let angles = coords.hue_angles();
        assert_eq!(angles.len(), 1);
        let expected = coords.chroma[1].atan2(coords.chroma[0]);
        assert!((angles[0] - expected).abs() < 1e-9);
    }

    #[test]
    fn monochromat_and_dichromat_have_no_hue_angles() {
        let illuminant = flat_curve("flat", 1.0);
        let stimulus = SpectralCurve::new("test", CurveType::Reflectance)
            .with_points(vec![(400.0, 0.5), (700.0, 0.5)]);

        let mono = Pipeline::build(
            {
                let mut set = CurveSet::new("Monochromat");
                set.colorspace_curves = vec![SpectralCurve::new("R", CurveType::Sensitivity)
                    .with_points(vec![(400.0, 1.0), (700.0, 1.0)])];
                set
            },
            &illuminant,
            1.0,
        );
        assert_eq!(mono.coordinates(&stimulus).hue_angles().len(), 0);

        let di = Pipeline::build(fixtures::dog(), &illuminant, 1.0);
        assert_eq!(di.coordinates(&stimulus).chroma.len(), 1);
        assert_eq!(di.coordinates(&stimulus).hue_angles().len(), 0);
    }

    #[test]
    fn human_pipeline_exposes_noise_from_fixture_data() {
        let illuminant = flat_curve("flat", 1.0);
        let pipeline = Pipeline::build(fixtures::human(), &illuminant, 1.0);
        let noise = pipeline
            .noise()
            .expect("human fixture has complete omega/eta");
        assert_eq!(noise.len(), 3);
        assert!(noise.iter().all(|n| n.is_finite() && *n > 0.0));
    }

    #[test]
    fn receptor_saturation_caps_strong_but_not_weak_stimuli() {
        // Flat receptor 2 x flat stimulus over 100nm: weak (value 1) ->
        // 200, strong (value 10) -> 2000. Cap at 500: weak passes through,
        // strong clamps.
        let receptor = SpectralCurve::new("R", CurveType::Sensitivity)
            .with_points(vec![(100.0, 2.0), (200.0, 2.0)])
            .with_saturation(500.0);
        let weak = SpectralCurve::new("weak", CurveType::Reflectance)
            .with_points(vec![(100.0, 1.0), (200.0, 1.0)]);
        let strong = SpectralCurve::new("strong", CurveType::Reflectance)
            .with_points(vec![(100.0, 10.0), (200.0, 10.0)]);
        let w = raw_activations(std::slice::from_ref(&receptor), &weak, 1.0)[0];
        let s = raw_activations(std::slice::from_ref(&receptor), &strong, 1.0)[0];
        assert!((w - 200.0).abs() < 1.0, "w={w}");
        assert_eq!(s, 500.0);
    }

    #[test]
    fn saturated_pipeline_still_produces_finite_coordinates() {
        let mut set = fixtures::human();
        for c in set.colorspace_curves.iter_mut() {
            c.saturation = Some(1.0);
        }
        let bright = flat_curve("bright", 1000.0);
        let pipeline = Pipeline::build(set, &bright, 1.0);
        let stimulus = flat_curve("stim", 1000.0);
        let coords = pipeline.coordinates(&stimulus);
        assert!(coords.luminance.is_finite());
        assert!(coords.chroma.iter().all(|c| c.is_finite()));
        // Every receptor is pinned at its cap, so the raw activations are
        // all exactly 1.0.
        assert!(pipeline
            .colorspace_activations(&stimulus)
            .iter()
            .all(|&q| q == 1.0));
    }

    #[test]
    fn raw_activations_free_function_matches_hand_calculation() {
        // Flat receptor (value 2) x flat stimulus (value 3) over the
        // 100nm-wide overlap [100,200] -> integral = 2*3*100 = 600.
        let receptor = SpectralCurve::new("R", CurveType::Sensitivity)
            .with_points(vec![(100.0, 2.0), (200.0, 2.0)]);
        let stimulus = SpectralCurve::new("stim", CurveType::Reflectance)
            .with_points(vec![(100.0, 3.0), (200.0, 3.0)]);
        let result = raw_activations(&[receptor], &stimulus, 1.0);
        assert_eq!(result.len(), 1);
        assert!((result[0] - 600.0).abs() < 1.0);
    }

    #[test]
    fn pipeline_colorspace_activations_differ_from_adapted_but_use_same_names() {
        let illuminant = flat_curve("flat", 1.0);
        let pipeline = Pipeline::build(fixtures::human(), &illuminant, 1.0);
        let stimulus = SpectralCurve::new("reddish", CurveType::Reflectance).with_points(vec![
            (300.0, 0.2),
            (500.0, 0.2),
            (600.0, 0.9),
            (700.0, 0.9),
        ]);
        let raw = pipeline.colorspace_activations(&stimulus);
        let adapted = pipeline.adapted_activations(&stimulus);
        assert_eq!(raw.len(), 3);
        assert_eq!(adapted.len(), 3);
        assert_ne!(raw, adapted, "adaptation should actually change the values");
        assert_eq!(
            pipeline.colorspace_curve_names(),
            vec!["S-cone", "M-cone", "L-cone"]
        );
    }

    #[test]
    fn pipeline_isolated_activations_computed_for_non_participating_curves() {
        // Butterfly has 4 colorspace + 2 isolated curves (violet, broad-band).
        let illuminant = flat_curve("flat", 1.0);
        let pipeline = Pipeline::build(fixtures::butterfly(), &illuminant, 1.0);
        let stimulus = SpectralCurve::new("test", CurveType::Reflectance)
            .with_points(vec![(300.0, 0.5), (750.0, 0.5)]);

        assert_eq!(pipeline.isolated_curve_names().len(), 2);
        let isolated = pipeline.isolated_activations(&stimulus);
        assert_eq!(isolated.len(), 2);
        assert!(isolated.iter().all(|v| v.is_finite()));

        // A direct call against the same curves must match exactly -
        // confirming isolated_activations isn't silently reusing the
        // colorspace curves' adaptation machinery.
        let violet = SpectralCurve::new("Violet (non-color-vision)", CurveType::Sensitivity)
            .with_points(crate::govardovskii::generate_points(
                400.0, 300.0, 750.0, 5.0,
            ));
        let direct = integrate_product(&violet, &stimulus, 1.0);
        assert!((isolated[0] - direct).abs() < 1e-6);
    }

    #[test]
    fn coordinates_adapted_activations_matches_direct_call() {
        let illuminant = flat_curve("flat", 1.0);
        let pipeline = Pipeline::build(fixtures::human(), &illuminant, 1.0);
        let stimulus = SpectralCurve::new("test", CurveType::Reflectance)
            .with_points(vec![(400.0, 0.5), (700.0, 0.5)]);
        let coords = pipeline.coordinates(&stimulus);
        let direct = pipeline.adapted_activations(&stimulus);
        assert_eq!(coords.adapted_activations, direct);
    }

    /// §7.2's "test at a higher N" exit criterion, through the
    /// end-to-end pipeline rather than just `build_chroma_axes`'s
    /// shape check above - an N=8 custom-style system (no species-
    /// specific code, same generic `Pipeline::build` every fixture uses)
    /// must still produce a correct, finite result.
    #[test]
    fn high_n_custom_system_still_produces_correct_chroma_count() {
        let n = 8;
        let colorspace_curves: Vec<SpectralCurve> = (0..n)
            .map(|i| {
                let lmax = 350.0 + i as f64 * (300.0 / (n as f64 - 1.0));
                SpectralCurve::new(format!("R{i}"), CurveType::Sensitivity).with_points(
                    crate::govardovskii::generate_points(lmax, 300.0, 750.0, 5.0),
                )
            })
            .collect();
        let mut set = CurveSet::new("High-N custom test system");
        set.colorspace_curves = colorspace_curves;

        let illuminant = flat_curve("flat", 1.0);
        let pipeline = Pipeline::build(set, &illuminant, 1.0);
        assert_eq!(pipeline.chroma_axis_count(), n - 1);

        let stimulus = SpectralCurve::new("test", CurveType::Reflectance)
            .with_points(vec![(400.0, 0.5), (700.0, 0.5)]);
        let coords = pipeline.coordinates(&stimulus);
        assert!(coords.luminance.is_finite());
        assert_eq!(coords.chroma.len(), n - 1);
        assert!(coords.chroma.iter().all(|c| c.is_finite()));
    }
}
