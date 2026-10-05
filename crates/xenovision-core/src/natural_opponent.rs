//! Derives opponent-contrast candidate rows (§4.2.4) from the receptor-
//! response covariance structure of a "natural scene" ensemble, rather
//! than requiring literature values or manual entry - grounded in the
//! finding (Buchsbaum & Gottschalk 1983; Ruderman, Cronin & Chiao 1998;
//! Wachtler, Lee & Sejnowski 2001) that principal components of cone
//! responses to natural scenes closely track the human L-M / S-(L+M)
//! opponent channels: those channels are themselves a fixed,
//! evolutionarily-cheap approximation to a solution that is actually
//! scene-statistics-dependent (§4.2.6).
//!
//! Two ways to supply the "natural scene" ensemble, both reducing to the
//! same final step (`contrasts_from_covariance`):
//! - `derive_from_parametric_model`: no external data, treating natural
//!   reflectance spectra as a smooth random process (motivated by the
//!   established finding that natural/Munsell reflectances are smooth
//!   and low-dimensional, e.g. Maloney 1986).
//! - `derive_from_corpus`: a user-supplied weighted set of
//!   reflectance/radiance curves (e.g. imported from a USGS spectral
//!   library via the Stimulus Editor).
//!
//! A single reference illuminant alone can't supply this: it rescales/
//! tints receptor responses but contributes no *variance across
//! stimuli* for PCA to decorrelate (one illuminant alone integrates to a
//! single response vector, whose covariance is zero). The variance has
//! to come from the ensemble of surfaces/pixels the illuminant is
//! applied to - that's what both modes below actually supply.

use crate::adaptation::sample_curves_on_common_grid;
use crate::curve::{QuantityKind, SpectralCurve};
use crate::curve_set::OpponentContrast;
use crate::illumination;
use crate::linalg::{dot, symmetric_eigen, Mat};
use crate::pipeline::raw_activations;

/// Turns an already-built receptor-response covariance matrix into
/// candidate opponent-contrast rows: one per eigenvector, in descending-
/// eigenvalue order (most natural-scene variance first - literature
/// finds this one tends to be luminance-like, which `build_chroma_axes`
/// then reduces towards zero when it orthogonalizes candidates against
/// the (separately-defined) luminance direction, same as it would for
/// any other candidate row). Each row is sign-canonicalized (its
/// largest-magnitude component made positive) so repeated runs on the
/// same input are reproducible rather than flipping sign arbitrarily.
///
/// Handles `n == 0`/`n == 1` the same trivial way
/// `derive_adaptation_matrix` does - there's nothing to decorrelate.
fn contrasts_from_covariance(cov: &Mat) -> Vec<OpponentContrast> {
    let n = cov.n;
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![OpponentContrast {
            name: "Natural-scene PC1".to_string(),
            weights: vec![1.0],
        }];
    }
    let (eigenvalues, eigenvectors) = symmetric_eigen(cov);
    eigenvectors
        .into_iter()
        .zip(eigenvalues.iter())
        .enumerate()
        .map(|(k, (mut weights, &eigenvalue))| {
            if let Some((idx, _)) = weights
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.abs().partial_cmp(&b.abs()).unwrap())
            {
                if weights[idx] < 0.0 {
                    for w in weights.iter_mut() {
                        *w = -*w;
                    }
                }
            }
            OpponentContrast {
                name: format!("Natural-scene PC{} (λ={eigenvalue:.4})", k + 1),
                weights,
            }
        })
        .collect()
}

/// Resolves a corpus/stimulus curve against `illuminant`, the same
/// reflectance×illuminant rule `window_comparison::resolve_stimulus`
/// applies: a Reflectance curve is a filter on the light, not light
/// itself, so it's multiplied by the illuminant first; anything else
/// (Radiance, or an unspecified/other kind) is assumed already
/// radiance-like and passed through unchanged.
fn resolve_against_illuminant(
    curve: &SpectralCurve,
    illuminant: &SpectralCurve,
    step_nm: f64,
) -> SpectralCurve {
    if curve.quantity == QuantityKind::Reflectance {
        illumination::predict_under_illuminant(curve, illuminant, step_nm)
    } else {
        curve.clone()
    }
}

/// Parametric mode (§4.2.6): models natural reflectance spectra as a
/// smooth random process with an exponential autocorrelation kernel
/// `K(λ,λ') = exp(-|λ-λ'|/ℓ)` - no embedded dataset, just the established
/// finding that natural/Munsell reflectances are smooth and low-
/// dimensional (Maloney 1986). With `T_i(λ) = S_i(λ)·E(λ)` (receptor `i`'s
/// sensitivity weighted by the reference illuminant `E`), the resulting
/// receptor-response covariance is the quadratic form
/// `Cov[i,j] = step_nm² · T_i · K · T_jᵗ`, computed exactly (no sampling
/// noise) on the curves' common wavelength grid.
pub fn derive_from_parametric_model(
    colorspace_curves: &[SpectralCurve],
    illuminant: &SpectralCurve,
    correlation_length_nm: f64,
    step_nm: f64,
) -> Vec<OpponentContrast> {
    let n = colorspace_curves.len();
    if n <= 1 {
        return contrasts_from_covariance(&Mat::zeros(n));
    }

    let mut all = colorspace_curves.to_vec();
    all.push(illuminant.clone());
    let grid = sample_curves_on_common_grid(&all, step_nm);
    let env = &grid[n];
    let t: Vec<Vec<f64>> = grid[..n]
        .iter()
        .map(|row| row.iter().zip(env).map(|(a, b)| a * b).collect())
        .collect();

    let grid_len = t.first().map(|row| row.len()).unwrap_or(0);
    // K·T_j for each receptor j, precomputed once (O(grid_len²) each)
    // rather than recomputed per (i,j) pair.
    let kt: Vec<Vec<f64>> = t
        .iter()
        .map(|tj| {
            (0..grid_len)
                .map(|a| {
                    (0..grid_len)
                        .map(|b| {
                            let dist = ((a as f64) - (b as f64)).abs() * step_nm;
                            (-dist / correlation_length_nm).exp() * tj[b]
                        })
                        .sum::<f64>()
                })
                .collect()
        })
        .collect();

    let mut cov = Mat::zeros(n);
    for (i, ti) in t.iter().enumerate() {
        for (j, ktj) in kt.iter().enumerate() {
            cov.set(i, j, step_nm * step_nm * dot(ti, ktj));
        }
    }
    contrasts_from_covariance(&cov)
}

/// One corpus member for `derive_from_corpus`: a reflectance/radiance
/// curve and its weight (relative prevalence in the scene - raw values
/// as entered, normalized internally, same convention as `eta`, §4.2.2).
pub struct CorpusEntry<'a> {
    pub curve: &'a SpectralCurve,
    pub weight: f64,
}

/// Custom-corpus mode (§4.2.6): the weighted empirical receptor-response
/// covariance of a user-supplied set of reflectance/radiance curves,
/// each resolved against `illuminant` first. `None` if there are
/// fewer than 2 entries or the total weight isn't positive - a
/// covariance needs variance across at least two differently-weighted
/// samples to be anything other than degenerate.
pub fn derive_from_corpus(
    colorspace_curves: &[SpectralCurve],
    illuminant: &SpectralCurve,
    corpus: &[CorpusEntry],
    step_nm: f64,
) -> Option<Vec<OpponentContrast>> {
    if corpus.len() < 2 {
        return None;
    }
    let total_weight: f64 = corpus.iter().map(|e| e.weight).sum();
    if total_weight <= 0.0 {
        return None;
    }

    let n = colorspace_curves.len();
    let responses: Vec<Vec<f64>> = corpus
        .iter()
        .map(|e| {
            let resolved = resolve_against_illuminant(e.curve, illuminant, step_nm);
            raw_activations(colorspace_curves, &resolved, step_nm)
        })
        .collect();

    let mean: Vec<f64> = (0..n)
        .map(|i| {
            corpus
                .iter()
                .zip(&responses)
                .map(|(e, r)| e.weight * r[i])
                .sum::<f64>()
                / total_weight
        })
        .collect();

    let mut cov = Mat::zeros(n);
    for i in 0..n {
        for j in 0..n {
            let v = corpus
                .iter()
                .zip(&responses)
                .map(|(e, r)| e.weight * (r[i] - mean[i]) * (r[j] - mean[j]))
                .sum::<f64>()
                / total_weight;
            cov.set(i, j, v);
        }
    }
    Some(contrasts_from_covariance(&cov))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::CurveType;
    use crate::fixtures;
    use crate::govardovskii;

    fn cone_curve(name: &str, lambda_max: f64) -> SpectralCurve {
        SpectralCurve::new(name, CurveType::Sensitivity)
            .with_points(govardovskii::generate_points(lambda_max, 300.0, 750.0, 5.0))
    }

    fn human_cones() -> Vec<SpectralCurve> {
        vec![
            cone_curve("S-cone", 420.0),
            cone_curve("M-cone", 530.0),
            cone_curve("L-cone", 560.0),
        ]
    }

    fn reflectance(name: &str, points: Vec<(f64, f64)>) -> SpectralCurve {
        SpectralCurve::new(name, CurveType::Reflectance)
            .with_points(points)
            .with_quantity(QuantityKind::Reflectance)
    }

    #[test]
    fn parametric_model_returns_n_unit_norm_contrasts() {
        let curves = human_cones();
        let illuminant = fixtures::default_solar_illuminant();
        let result = derive_from_parametric_model(&curves, &illuminant, 50.0, 1.0);
        assert_eq!(result.len(), 3);
        for c in &result {
            let norm_sq: f64 = c.weights.iter().map(|w| w * w).sum();
            assert!((norm_sq - 1.0).abs() < 1e-6, "{} not unit norm", c.name);
        }
    }

    #[test]
    fn parametric_model_top_component_is_same_sign_luminance_like() {
        // Per Buchsbaum/Ruderman/Wachtler: the dominant (highest-
        // variance) component of natural-scene receptor responses tends
        // to be achromatic/luminance-like - same sign across all
        // receptors - for broadly-overlapping curves like human cones.
        let curves = human_cones();
        let illuminant = fixtures::default_solar_illuminant();
        let result = derive_from_parametric_model(&curves, &illuminant, 50.0, 1.0);
        let top = &result[0].weights;
        assert!(
            top.iter().all(|w| *w > 0.0) || top.iter().all(|w| *w < 0.0),
            "top component {top:?} isn't same-signed across all three cones"
        );
    }

    #[test]
    fn corpus_mode_requires_at_least_two_entries() {
        let curves = human_cones();
        let illuminant = fixtures::default_solar_illuminant();
        let r = reflectance("r", vec![(400.0, 0.5), (700.0, 0.5)]);
        let corpus = [CorpusEntry {
            curve: &r,
            weight: 1.0,
        }];
        assert!(derive_from_corpus(&curves, &illuminant, &corpus, 1.0).is_none());
        assert!(derive_from_corpus(&curves, &illuminant, &[], 1.0).is_none());
    }

    #[test]
    fn corpus_mode_requires_positive_total_weight() {
        let curves = human_cones();
        let illuminant = fixtures::default_solar_illuminant();
        let a = reflectance("a", vec![(400.0, 0.8), (700.0, 0.2)]);
        let b = reflectance("b", vec![(400.0, 0.2), (700.0, 0.8)]);
        let corpus = [
            CorpusEntry {
                curve: &a,
                weight: 1.0,
            },
            CorpusEntry {
                curve: &b,
                weight: -1.0,
            },
        ];
        assert!(derive_from_corpus(&curves, &illuminant, &corpus, 1.0).is_none());
    }

    #[test]
    fn corpus_mode_returns_n_unit_norm_contrasts_for_a_diverse_corpus() {
        let curves = human_cones();
        let illuminant = fixtures::default_solar_illuminant();
        let reddish = reflectance(
            "reddish",
            vec![(400.0, 0.2), (500.0, 0.2), (600.0, 0.8), (700.0, 0.8)],
        );
        let greenish = reflectance(
            "greenish",
            vec![(400.0, 0.2), (530.0, 0.8), (600.0, 0.2), (700.0, 0.2)],
        );
        let blueish = reflectance(
            "blueish",
            vec![(400.0, 0.8), (450.0, 0.8), (550.0, 0.2), (700.0, 0.2)],
        );
        let corpus = [
            CorpusEntry {
                curve: &reddish,
                weight: 1.0,
            },
            CorpusEntry {
                curve: &greenish,
                weight: 1.0,
            },
            CorpusEntry {
                curve: &blueish,
                weight: 1.0,
            },
        ];
        let result = derive_from_corpus(&curves, &illuminant, &corpus, 1.0).unwrap();
        assert_eq!(result.len(), 3);
        for c in &result {
            let norm_sq: f64 = c.weights.iter().map(|w| w * w).sum();
            assert!((norm_sq - 1.0).abs() < 1e-6, "{} not unit norm", c.name);
        }
    }
}
