//! Photoreceptor-to-perceptual-channel transform (design doc §2.2):
//! receptor activation, chromatic adaptation (via `adaptation`), and the
//! luminance + opponent (chroma) construction, combined into a per-
//! species+illuminant `Pipeline` that's cheap to evaluate per stimulus.
//!
//! A `Pipeline` now evaluates *one colorspace regime* (§2.2): build one
//! per regime from its `RegimeConfig` (`CurveSet::regime_config`). A
//! species with several regimes (e.g. the frog's scotopic/mesopic/
//! photopic, §2.3.10) produces several pipelines; species with no
//! regimes at all (§10.2's Mantis Shrimp) never build one, their
//! receptors being reported individually instead.

use crate::adaptation::{derive_adaptation_matrix, AdaptationResult};
use crate::curve::{operating_gain, SpectralCurve, DEFAULT_ILLUMINATION_SOFTNESS};
use crate::curve_set::RegimeConfig;
use crate::linalg::{self, dot, norm, Mat};

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
/// same formula `Pipeline` already uses internally for its regime member
/// curves before adaptation, exposed generically here so it applies
/// identically to curves that never enter the opponent-process pipeline
/// at all (§10.1, §10.2: receptors not in any colorspace regime, e.g.
/// the swallowtail butterfly's violet/broad-band receptors or - the
/// no-regime-at-all case - a Mantis Shrimp fixture's entire receptor
/// set). There's no "adapted" version of this for unaffiliated curves to
/// show instead: adaptation is itself a property of a regime's
/// chromatic-adaptation matrix, which such a curve was never part of
/// deriving.
pub fn raw_activations(
    curves: &[SpectralCurve],
    stimulus: &SpectralCurve,
    step_nm: f64,
) -> Vec<f64> {
    let gains = vec![1.0; curves.len()];
    raw_activations_with_gains(curves, &gains, stimulus, step_nm)
}

/// As `raw_activations`, but each receptor's raw integral is first scaled
/// by its operating gain `g_i` (§2.2, receptor operating window) *before*
/// the saturation cap. Scaling the light available to the receptor (rather
/// than the post-cap signal) is what makes `saturation` a ceiling on the
/// transduced response: at a lower operating gain, more incident power is
/// needed to reach it. `gains` is indexed in parallel with `curves`; a
/// missing entry is treated as 1.0 (always on).
pub fn raw_activations_with_gains(
    curves: &[SpectralCurve],
    gains: &[f64],
    stimulus: &SpectralCurve,
    step_nm: f64,
) -> Vec<f64> {
    curves
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let g = gains.get(i).copied().unwrap_or(1.0);
            c.saturate(g * integrate_product(c, stimulus, step_nm))
        })
        .collect()
}

/// Per-receptor seen irradiances (W/m²) for every curve in `curves`
/// under `illuminant`: `E_i = ∫ S_i(λ)·I(λ) dλ` (§2.2) - the
/// receptor-filtered ambient light each receptor actually absorbs. The
/// pool-wide counterpart of `Pipeline::seen_irradiances`, used to report
/// receptors that belong to no colorspace regime (§10.1/§10.2) with
/// their own operating state even though they never enter an opponent
/// pipeline. The caller owns gating (`CurveSet::receptor_gains`).
pub fn seen_irradiances_for(
    curves: &[SpectralCurve],
    illuminant: &SpectralCurve,
    step_nm: f64,
) -> Vec<f64> {
    curves
        .iter()
        .map(|c| integrate_product(c, illuminant, step_nm))
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

/// A fully-built per-regime, per-reference-illuminant pipeline (§2.2's
/// combined transform restricted to one colorspace regime): the expensive
/// part (deriving the adaptation matrix, §2.2.3) is done once in `build`,
/// so `coordinates` is cheap to call per stimulus. Build one per regime
/// of the species; regime ordering across pipelines is the species'
/// `regimes` list order.
pub struct Pipeline {
    /// The regime this pipeline evaluates, from its config's name.
    name: String,
    /// This regime's member curves, in member order.
    curves: Vec<SpectralCurve>,
    adaptation: AdaptationResult,
    q_env: Vec<f64>,
    luminance_weights: Vec<f64>,
    chroma_axes: Vec<Vec<f64>>,
    /// Vorobyev-Osorio per-member noise terms (§3.3.2), if every member
    /// had complete omega+eta data - see `RegimeConfig.noise`.
    noise: Option<Vec<f64>>,
    step_nm: f64,
    /// Total irradiance (W/m²) of the reference illuminant
    /// (`illumination::irradiance_w_m2`) - the *unfiltered* total the
    /// standard luminant emits. Informational (the Comparison window
    /// shows it); gating itself uses the per-receptor seen irradiances
    /// below, because receptors only "see" the light they absorb.
    reference_irradiance_w_m2: f64,
    /// Irradiance (W/m²) each regime member actually sees under the
    /// reference illuminant: `E_i = ∫ S_i(λ)·I(λ) dλ` (§2.2) - the
    /// receptor-filtered ambient light that drives `response_gains`.
    seen_irradiances: Vec<f64>,
    /// Operating gain `g_i` for each member curve, in order (§2.2,
    /// receptor operating window), from that curve's own seen
    /// irradiance. Applied to raw activations before adaptation so the
    /// adapting white point is consistent.
    response_gains: Vec<f64>,
    /// The regime's overall activity: the product of its members' gains
    /// (`∏ g_i`, §2.2). ~1 only where *every* member's window contains
    /// its seen irradiance - so a dim-only regime is ~0 in bright light,
    /// and a mesopic regime (rods *and* cones both on, §2.3.10) peaks
    /// only in the band where both receptor classes operate.
    regime_gain: f64,
}

impl Pipeline {
    /// Builds the pipeline for one colorspace regime from the config the
    /// `CurveSet` resolved for it (`CurveSet::regime_config`): luminance
    /// weights, opponent-contrast candidate rows, per-member windows, and
    /// ΔS noise terms all come from the set's own data (§4's
    /// representability goal - the data model carries everything the
    /// pipeline needs, with no separately-maintained parameter vectors
    /// that could drift out of sync with it).
    pub fn build(config: RegimeConfig, reference_illuminant: &SpectralCurve, step_nm: f64) -> Self {
        let RegimeConfig {
            name,
            luminance_weights,
            candidate_rows,
            noise,
            curves,
            illumination_min,
            illumination_max,
            illumination_softness,
        } = config;

        // Each member's operating gain (§2.2) is driven by the irradiance
        // that receptor actually absorbs from the reference illuminant -
        // not the unfiltered total - because a receptor "sees" only the
        // light its own sensitivity passes: E_i = ∫ S_i(λ)·I(λ)dλ in
        // W/m² (luminants are spectral irradiance in W/m²/nm,
        // sensitivities are dimensionless weights).
        let reference_irradiance_w_m2 =
            crate::illumination::irradiance_w_m2(reference_illuminant, step_nm);
        let seen_irradiances: Vec<f64> = curves
            .iter()
            .map(|c| integrate_product(c, reference_illuminant, step_nm))
            .collect();
        let response_gains: Vec<f64> = curves
            .iter()
            .zip(seen_irradiances.iter())
            .map(|(c, &seen)| {
                operating_gain(
                    seen,
                    c.illumination_min.or(illumination_min),
                    c.illumination_max.or(illumination_max),
                    c.illumination_softness
                        .or(illumination_softness)
                        .unwrap_or(DEFAULT_ILLUMINATION_SOFTNESS),
                )
            })
            .collect();
        let regime_gain = response_gains.iter().product();

        let adaptation = derive_adaptation_matrix(&curves, step_nm);
        let q_env: Vec<f64> =
            raw_activations_with_gains(&curves, &response_gains, reference_illuminant, step_nm);
        let chroma_axes = build_chroma_axes(&candidate_rows, &luminance_weights, 1e-9);

        Pipeline {
            name,
            curves,
            adaptation,
            q_env,
            luminance_weights,
            chroma_axes,
            noise,
            step_nm,
            reference_irradiance_w_m2,
            seen_irradiances,
            response_gains,
            regime_gain,
        }
    }

    /// The regime this pipeline evaluates (`RegimeConfig.name`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The total irradiance (W/m²) of the reference illuminant this
    /// pipeline was built against (§2.2) - the unfiltered total, for
    /// display; gating itself uses each receptor's seen irradiance.
    pub fn reference_irradiance_w_m2(&self) -> f64 {
        self.reference_irradiance_w_m2
    }

    /// The irradiance (W/m²) each regime member actually absorbs from the
    /// reference illuminant, in the same order as `response_gains`/
    /// `activations` (§2.2) - the receptor-filtered ambient light that
    /// set its operating gain.
    pub fn seen_irradiances(&self) -> &[f64] {
        &self.seen_irradiances
    }

    /// Operating gain of each regime member, in the same order as
    /// `curve_names`/`activations` (§2.2).
    pub fn response_gains(&self) -> &[f64] {
        &self.response_gains
    }

    /// The regime's overall activity, `∏ g_i` over its members' gains
    /// (§2.2): ~1 only where every member's own window contains its seen
    /// irradiance. Used to compare regimes side-by-side and mark the
    /// dominant one at a given light level. A degenerate regime with no
    /// members has gain 1.0 but `is_active() == false` (nothing
    /// operating).
    pub fn regime_gain(&self) -> f64 {
        self.regime_gain
    }

    /// `true` if at least one regime member is substantially operating
    /// (gain above 0.1) under this pipeline's reference illuminant. A
    /// regime with no operating receptors produces all-zero coordinates;
    /// callers use this to flag that state rather than present it as a
    /// real result. The 0.1 threshold means a receptor merely sitting on
    /// the shoulder of its window (gain < 10%) does not count as
    /// operating.
    pub fn is_active(&self) -> bool {
        self.response_gains.iter().any(|g| *g > 0.1)
    }

    pub fn adaptation_result(&self) -> &AdaptationResult {
        &self.adaptation
    }

    pub fn chroma_axis_count(&self) -> usize {
        self.chroma_axes.len()
    }

    /// This regime's luminance-weight coefficient row, in member order -
    /// the row `coordinates` projects adapted activations onto for `L`.
    pub fn luminance_weights(&self) -> &[f64] {
        &self.luminance_weights
    }

    /// This regime's chroma axes (each a coefficient row in member
    /// order), orthogonalized against luminance and each other.
    /// `chroma_axis_count` is just `chroma_axes().len()`.
    pub fn chroma_axes(&self) -> &[Vec<f64>] {
        &self.chroma_axes
    }

    pub fn noise(&self) -> Option<&[f64]> {
        self.noise.as_deref()
    }

    /// The composed adaptation map `A` such that `Q' = A·Q` turns raw
    /// (post-gain, post-saturation) activations into adapted activations
    /// in one matrix multiply - the linear equivalent of
    /// [`Pipeline::adapt`]'s `M⁻¹·diag(1/(M·q_env))·M`, precomputed so
    /// callers that adapt many stimuli at once (the receptor-image
    /// renderers) pay the per-channel setup once per build instead of
    /// once per stimulus.
    pub fn adaptation_transform(&self) -> Mat {
        let n = self.adaptation.matrix.n;
        let aq_env = self.adaptation.matrix.mul_vec(&self.q_env);
        // diag(1/aq_env), where a near-zero environment channel maps to
        // 1.0 (no scaling) - matching `adapt`'s per-element guard.
        let mut scaled = Mat::zeros(n);
        for (i, &e) in aq_env.iter().enumerate() {
            let s = if e.abs() < 1e-12 { 1.0 } else { 1.0 / e };
            for j in 0..n {
                scaled.set(i, j, s * self.adaptation.matrix.get(i, j));
            }
        }
        self.adaptation.inverse.mul_mat(&scaled)
    }

    /// Adapted receptor activations `Q'` for raw activations `q`, via the
    /// von Kries-style diagonal scaling in adaptation space (§2.2.3 step
    /// 2): transform into adaptation space, scale each channel by the
    /// inverse of its response to the reference illuminant, transform
    /// back.
    fn adapt(&self, q: &[f64]) -> Vec<f64> {
        self.adaptation_transform().mul_vec(q)
    }

    /// Adapted receptor activations `Q'` for a stimulus spectrum - the
    /// same quantity `coordinates` derives luminance/chroma from, exposed
    /// directly for the ΔS metric (§3.3.2).
    pub fn adapted_activations(&self, stimulus: &SpectralCurve) -> Vec<f64> {
        self.adapt(&self.activations(stimulus))
    }

    /// Raw (pre-adaptation) activations for this regime's member curves
    /// (§10.1) - the same `Q` `adapted_activations` adapts, exposed on
    /// its own so a UI can display both the raw and adapted value per
    /// receptor rather than only the adapted one. Includes each member's
    /// operating gain (§2.2).
    pub fn activations(&self, stimulus: &SpectralCurve) -> Vec<f64> {
        raw_activations_with_gains(&self.curves, &self.response_gains, stimulus, self.step_nm)
    }

    /// Display names for this regime's member curves, in the same order
    /// `activations`/`adapted_activations`/`coordinates` use.
    pub fn curve_names(&self) -> Vec<&str> {
        self.curves.iter().map(|c| c.name.as_str()).collect()
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
    use crate::curve_set::CurveSet;
    use crate::fixtures;

    fn flat_curve(name: &str, value: f64) -> SpectralCurve {
        SpectralCurve::new(name, CurveType::Illumination)
            .with_points(vec![(300.0, value), (750.0, value)])
    }

    /// A single-regime species' pipeline: the common test shape.
    fn build_default(set: CurveSet, illuminant: &SpectralCurve) -> Pipeline {
        Pipeline::build(set.default_regime_config(), illuminant, 1.0)
    }

    fn build_regime(set: &CurveSet, idx: usize, illuminant: &SpectralCurve) -> Pipeline {
        Pipeline::build(set.regime_config(&set.regimes[idx]), illuminant, 1.0)
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
            assert_eq!(
                chroma.len(),
                n - 1,
                "n={n} generic candidates must reduce to n-1 axes"
            );
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
        let config = human.regime_config(&human.regimes[0]);
        let luminance = config.luminance_weights;
        let candidates = config.candidate_rows;
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
        let pipeline = build_default(fixtures::human(), &illuminant);
        assert_eq!(pipeline.chroma_axis_count(), 2);
    }

    #[test]
    fn human_pipeline_coordinates_are_finite_and_saturation_matches_chroma() {
        let illuminant = flat_curve("flat", 1.0);
        let pipeline = build_default(fixtures::human(), &illuminant);
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
        let pipeline = build_default(fixtures::human(), &illuminant);
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

        let mono = build_default(
            {
                let mut set = CurveSet::new("Monochromat");
                set.curves = vec![SpectralCurve::new("R", CurveType::Sensitivity)
                    .with_points(vec![(400.0, 1.0), (700.0, 1.0)])];
                set.regimes = vec![crate::curve_set::Regime::new("Mono", vec![0])];
                set
            },
            &illuminant,
        );
        assert_eq!(mono.coordinates(&stimulus).hue_angles().len(), 0);

        let di = build_default(fixtures::dog(), &illuminant);
        assert_eq!(di.coordinates(&stimulus).chroma.len(), 1);
        assert_eq!(di.coordinates(&stimulus).hue_angles().len(), 0);
    }

    #[test]
    fn human_pipeline_exposes_noise_from_fixture_data() {
        let illuminant = flat_curve("flat", 1.0);
        let pipeline = build_default(fixtures::human(), &illuminant);
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
        for c in set.curves.iter_mut() {
            c.saturation = Some(1.0);
        }
        let bright = flat_curve("bright", 1000.0);
        let pipeline = build_default(set, &bright);
        let stimulus = flat_curve("stim", 1000.0);
        let coords = pipeline.coordinates(&stimulus);
        assert!(coords.luminance.is_finite());
        assert!(coords.chroma.iter().all(|c| c.is_finite()));
        // Every receptor is pinned at its cap, so the raw activations are
        // all exactly 1.0.
        assert!(pipeline.activations(&stimulus).iter().all(|&q| q == 1.0));
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
    fn pipeline_activations_differ_from_adapted_but_use_same_names() {
        let illuminant = flat_curve("flat", 1.0);
        let pipeline = build_default(fixtures::human(), &illuminant);
        let stimulus = SpectralCurve::new("reddish", CurveType::Reflectance).with_points(vec![
            (300.0, 0.2),
            (500.0, 0.2),
            (600.0, 0.9),
            (700.0, 0.9),
        ]);
        let raw = pipeline.activations(&stimulus);
        let adapted = pipeline.adapted_activations(&stimulus);
        assert_eq!(raw.len(), 3);
        assert_eq!(adapted.len(), 3);
        assert_ne!(raw, adapted, "adaptation should actually change the values");
        assert_eq!(pipeline.curve_names(), vec!["S-cone", "M-cone", "L-cone"]);
    }

    #[test]
    fn unaffiliated_receptors_report_their_own_operating_state() {
        // Butterfly has 4 regimes-member receptors + 2 unaffiliated
        // (violet, broad-band). The unaffiliated ones never enter an
        // opponent pipeline, but pool-wide reporting still gives them
        // their own seen irradiances and gains.
        let set = fixtures::butterfly();
        let illuminant = flat_curve("flat", 1.0);
        let seen = seen_irradiances_for(&set.curves, &illuminant, 1.0);
        assert_eq!(seen.len(), 6, "the whole pool reports, not just members");
        let gains = set.receptor_gains(&seen);
        assert_eq!(gains.len(), 6);

        // Violet + broad-band carry no windows -> always on (gain ~1).
        assert_eq!(set.curves[4].name, "Violet (non-color-vision)");
        assert!(
            (gains[4] - 1.0).abs() < 1e-12 && (gains[5] - 1.0).abs() < 1e-12,
            "unaffiliated receptors are ungated: {gains:?}"
        );

        // Its seen irradiance is the plain integral against the reference
        // illuminant, matching a direct call - no adaptation machinery
        // involved for a receptor outside any regime.
        let violet = SpectralCurve::new("Violet (non-color-vision)", CurveType::Sensitivity)
            .with_points(crate::govardovskii::generate_points(
                400.0, 300.0, 750.0, 5.0,
            ));
        let direct_seen = integrate_product(&violet, &illuminant, 1.0);
        assert!(
            (seen[4] - direct_seen).abs() < 1e-6,
            "seen {} vs direct {direct_seen}",
            seen[4]
        );

        // And its raw activation under a stimulus is equally plain.
        let stimulus = SpectralCurve::new("test", CurveType::Reflectance)
            .with_points(vec![(300.0, 0.5), (750.0, 0.5)]);
        let raw =
            crate::pipeline::raw_activations(std::slice::from_ref(&set.curves[4]), &stimulus, 1.0)
                [0];
        let direct_raw = integrate_product(&violet, &stimulus, 1.0);
        assert!(
            (raw - direct_raw).abs() < 1e-6,
            "activation {raw} vs direct {direct_raw}"
        );
    }

    #[test]
    fn coordinates_adapted_activations_matches_direct_call() {
        let illuminant = flat_curve("flat", 1.0);
        let pipeline = build_default(fixtures::human(), &illuminant);
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
        let curves: Vec<SpectralCurve> = (0..n)
            .map(|i| {
                let lmax = 350.0 + i as f64 * (300.0 / (n as f64 - 1.0));
                SpectralCurve::new(format!("R{i}"), CurveType::Sensitivity).with_points(
                    crate::govardovskii::generate_points(lmax, 300.0, 750.0, 5.0),
                )
            })
            .collect();
        let mut set = CurveSet::new("High-N custom test system");
        set.curves = curves;
        set.regimes = vec![crate::curve_set::Regime::new(
            "High-N",
            (0..n as u64).collect(),
        )];

        let illuminant = flat_curve("flat", 1.0);
        let pipeline = build_default(set, &illuminant);
        assert_eq!(pipeline.chroma_axis_count(), n - 1);

        let stimulus = SpectralCurve::new("test", CurveType::Reflectance)
            .with_points(vec![(400.0, 0.5), (700.0, 0.5)]);
        let coords = pipeline.coordinates(&stimulus);
        assert!(coords.luminance.is_finite());
        assert_eq!(coords.chroma.len(), n - 1);
        assert!(coords.chroma.iter().all(|c| c.is_finite()));
    }

    /// §2.2's regime-gating example at the pipeline level: two receptors
    /// with disjoint operating windows (e.g. a dim-light rod and a
    /// bright-light cone) - only the receptor whose *own window contains
    /// the irradiance it actually sees* under the current light
    /// contributes. Each receptor sees `E_i = ∫ S_i(λ)·I(λ)dλ` (W/m²
    /// under the W/m²/nm luminant convention): both probe curves here
    /// span 70 nm·w, so with the flat illuminant at amplitude `power`,
    /// E = 70·power.
    #[test]
    fn disjoint_operating_windows_switch_regime_with_light_level() {
        // Receptor A operates at dim seen irradiances [1e-3, 1e-1]; B at
        // bright [10, 1e3] (W/m²). Never overlap. Modeled as two
        // single-member regimes: the species is in the dim regime in dim
        // light and the bright regime in bright light.
        let mut set = CurveSet::new("Regime pair");
        set.curves = vec![
            SpectralCurve::new("Dim", CurveType::Sensitivity)
                .with_points(vec![(400.0, 1.0), (500.0, 0.4)])
                .with_illumination_window(Some(1e-3), Some(1e-1))
                .with_illumination_softness(0.25),
            SpectralCurve::new("Bright", CurveType::Sensitivity)
                .with_points(vec![(400.0, 0.4), (500.0, 1.0)])
                .with_illumination_window(Some(1e1), Some(1e3))
                .with_illumination_softness(0.25),
        ];
        set.regimes = vec![
            crate::curve_set::Regime::new("Dim regime", vec![0]),
            crate::curve_set::Regime::new("Bright regime", vec![1]),
        ];

        let gain_at = |power: f64| {
            let illuminant = {
                let mut e = flat_curve("e", 1.0);
                for pt in e.points.iter_mut() {
                    pt.1 = power;
                }
                e
            };
            let dim = build_regime(&set, 0, &illuminant);
            let bright = build_regime(&set, 1, &illuminant);
            (
                dim.seen_irradiances()[0],
                dim.response_gains()[0],
                bright.response_gains()[0],
                dim.regime_gain(),
                bright.regime_gain(),
            )
        };

        // power 1e-4 -> E ≈ 7e-3 W/m² (70·power plus a tiny edge bias
        // from the sensitivity curve's hard domain cutoffs), mid-A-window.
        let (seen, dim_gain, bright_gain, dim_regime_gain, bright_regime_gain) = gain_at(1e-4);
        assert!(
            (seen / 7e-3 - 1.0).abs() < 0.02,
            "E_A ≈ 70·power, got {seen}"
        );
        assert!(
            dim_gain > 0.99 && bright_gain < 1e-3,
            "dim light should activate A only, got dim={dim_gain} bright={bright_gain}"
        );
        assert!(
            dim_regime_gain > 0.99 && bright_regime_gain < 1e-3,
            "the dim regime dominates in dim light: {dim_regime_gain} vs {bright_regime_gain}"
        );
        // power 1.0 -> E = 70 W/m², mid-B-window.
        let (_, dim_gain, bright_gain, dim_regime_gain, bright_regime_gain) = gain_at(1.0);
        assert!(
            dim_gain < 1e-3 && bright_gain > 0.99,
            "bright light should activate B only, got dim={dim_gain} bright={bright_gain}"
        );
        assert!(
            bright_regime_gain > 0.99 && dim_regime_gain < 1e-3,
            "the bright regime dominates in bright light: {bright_regime_gain} vs {dim_regime_gain}"
        );
        // power 0.02 -> E = 1.4 W/m², in the gap between the windows.
        let (_, dim_gain, bright_gain, dim_regime_gain, bright_regime_gain) = gain_at(0.02);
        assert!(
            dim_gain < 1e-3 && bright_gain < 1e-3,
            "in the gap both receptors should be silent, got dim={dim_gain} bright={bright_gain}"
        );
        assert!(
            dim_regime_gain < 1e-3 && bright_regime_gain < 1e-3,
            "no regime is active in the gap: {dim_regime_gain} vs {bright_regime_gain}"
        );
    }

    /// §2.2's `regime_gain = ∏ g_i` over member gains: a two-member
    /// regime is ~1 only mid-window for *both* members, and the stored
    /// regime gain is exactly the product of the member response gains.
    #[test]
    fn regime_gain_is_the_product_of_member_gains() {
        let mut set = CurveSet::new("Two-member regime");
        set.curves = vec![
            SpectralCurve::new("A", CurveType::Sensitivity)
                .with_points(vec![(400.0, 1.0), (500.0, 0.4)])
                .with_illumination_window(Some(1e-3), Some(1e-1))
                .with_illumination_softness(0.5),
            SpectralCurve::new("B", CurveType::Sensitivity)
                .with_points(vec![(400.0, 0.4), (500.0, 1.0)])
                .with_illumination_window(Some(1e-3), Some(1e-1))
                .with_illumination_softness(0.5),
        ];
        set.regimes = vec![crate::curve_set::Regime::new("Pair", vec![0, 1])];
        // Each probe curve spans 70 nm·w, so at power 1.5e-4 both see
        // E ≈ 70·1.5e-4 ≈ 1e-2 W/m² - the geometric mid-point of
        // [1e-3, 1e-1], an equal margin off either shoulder.
        let illuminant = {
            let mut e = flat_curve("e", 1.5e-4);
            for pt in e.points.iter_mut() {
                pt.1 = 1.5e-4;
            }
            e
        };
        let pipeline = build_default(set, &illuminant);
        let product: f64 = pipeline.response_gains().iter().product();
        assert!(
            (pipeline.regime_gain() - product).abs() < 1e-9,
            "regime_gain must be exactly ∏ g_i"
        );
        assert!(
            product > 0.9,
            "both members mid-window -> regime nearly fully on: {product}"
        );

        // A pair whose members' windows don't overlap (dim member
        // [1e-3, 1e-1], interloper [1e1, 1e3]): at E ≈ 1e-2 W/m² the dim
        // member is mid-window and the interloper is far below its 1e1
        // min. The regime's product is ~0 even though *a* member is
        // operating - the regime's activity is the joint condition, not
        // the strongest member (this is exactly why the mesopic regime
        // collapses outside its band, §2.3.10).
        let mut pair = CurveSet::new("Pair");
        pair.curves = vec![
            SpectralCurve::new("Dim member", CurveType::Sensitivity)
                .with_points(vec![(400.0, 1.0), (500.0, 0.4)])
                .with_illumination_window(Some(1e-3), Some(1e-1))
                .with_illumination_softness(0.5),
            SpectralCurve::new("Interloper", CurveType::Sensitivity)
                .with_points(vec![(400.0, 0.4), (500.0, 1.0)])
                .with_illumination_window(Some(1e1), Some(1e3))
                .with_illumination_softness(0.5),
        ];
        pair.regimes = vec![crate::curve_set::Regime::new("Combined", vec![0, 1])];
        let mid = {
            let mut e = flat_curve("mid", 1.5e-4);
            for pt in e.points.iter_mut() {
                pt.1 = 1.5e-4;
            }
            e
        };
        let combined = build_default(pair.clone(), &mid);
        assert!(
            combined.is_active() && combined.regime_gain() < 1e-3,
            "a member is on but the joint product is ~0: gain={}",
            combined.regime_gain()
        );

        // The same dim member alone, as its own single-member regime at
        // the same light: ~0.98. One pipeline cannot tell this apart
        // from a regime that's genuinely all-on - only the per-regime
        // product can.
        let mut dim_only = CurveSet::new("Dim only");
        dim_only.curves = vec![pair.curves[0].clone()];
        dim_only.regimes = vec![crate::curve_set::Regime::new("Dim", vec![0])];
        let single = build_default(dim_only, &mid);
        assert!(
            single.regime_gain() > 0.9 && single.is_active(),
            "single-member regime is nearly fully on: {}",
            single.regime_gain()
        );
    }

    #[test]
    fn gated_pipeline_produces_finite_all_zero_coordinates_when_inactive() {
        // Both receptors in the dim window (max 1e-1 W/m² seen), ambient
        // flat-10 (E = 70·10 = 700 W/m² seen) way above it: every
        // receptor gated off. Must be finite ~zeros, not NaN, and the
        // pipeline must report itself inactive.
        let mut set = CurveSet::new("Only-dim");
        set.curves = vec![
            SpectralCurve::new("A", CurveType::Sensitivity)
                .with_points(vec![(400.0, 1.0), (500.0, 0.4)])
                .with_illumination_window(Some(1e-3), Some(1e-1)),
            SpectralCurve::new("B", CurveType::Sensitivity)
                .with_points(vec![(400.0, 0.4), (500.0, 1.0)])
                .with_illumination_window(Some(1e-3), Some(1e-1)),
        ];
        set.regimes = vec![crate::curve_set::Regime::new("Dim", vec![0, 1])];
        let bright = flat_curve("bright", 10.0);
        let pipeline = build_default(set, &bright);
        assert!(!pipeline.is_active());
        let stimulus = flat_curve("test", 1.0);
        let coords = pipeline.coordinates(&stimulus);
        assert!(coords.luminance.is_finite());
        assert!(coords.chroma.iter().all(|c| c.is_finite()));
        assert!(
            pipeline
                .activations(&stimulus)
                .iter()
                .all(|q| q.abs() < 1e-3),
            "gated-off receptors must yield ~zero raw activations"
        );
    }

    #[test]
    fn operating_gain_applies_before_the_saturation_cap() {
        // Gain 0.1 * raw 1000 = 100, exactly the cap. If the gain were
        // applied after the cap, the result would be 10 (cap first).
        let receptor = SpectralCurve::new("R", CurveType::Sensitivity)
            .with_points(vec![(100.0, 2.0), (200.0, 2.0)]) // x flat 5 -> 1000 raw
            .with_saturation(100.0);
        let stimulus = SpectralCurve::new("stim", CurveType::Reflectance)
            .with_points(vec![(100.0, 5.0), (200.0, 5.0)]);
        let q = raw_activations_with_gains(&[receptor], &[0.1], &stimulus, 1.0)[0];
        assert!((q - 100.0).abs() < 1.0, "q={q} (gain then cap)");
    }
}
