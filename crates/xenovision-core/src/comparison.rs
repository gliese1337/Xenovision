//! Multi-spectrum perceptual coordinate comparison (design doc §3.3):
//! pairwise distance metrics and the resulting difference matrix.
//!
//! §3.3's shared-reference-environment constraint - all spectra in a
//! given table/matrix must share one reference illuminant - isn't
//! something this module enforces itself; it falls out naturally from
//! using one `Pipeline` (built against one illuminant) to compute every
//! `Coordinates` that goes into `difference_matrix`.

use crate::curve::SpectralCurve;
use crate::curve_set::CurveSet;
use crate::pipeline::{Coordinates, Pipeline};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistanceMetric {
    /// `sqrt((L_A-L_B)^2 + sum((C_i,A - C_i,B)^2))` (§3.3.2 #1).
    Euclidean,
    /// `sqrt(sum((C_i,A - C_i,B)^2))`, excluding luminance (§3.3.2 #2).
    ChromaOnly,
    /// Vorobyev-Osorio receptor-noise-limited distance (§3.3.2 #3),
    /// operating on adapted receptor activations directly rather than
    /// luminance/chroma. Needs a per-receptor noise vector
    /// (`CurveSet::receptor_noise`) to actually compute - see `distance`.
    DeltaS,
}

/// Distance between two coordinate vectors under `metric`. `a` and `b`
/// must have the same chroma dimensionality (i.e. come from the same
/// species' pipeline).
///
/// `noise` is the per-receptor Vorobyev-Osorio noise vector
/// (`CurveSet::receptor_noise`/`Pipeline::noise`) and is only consulted
/// for `DistanceMetric::DeltaS`; `distance` returns `None` for that
/// metric if `noise` is absent or doesn't match `a`/`b`'s receptor count.
///
/// ΔS formula: the design doc specifies the per-receptor noise term
/// `e_i = ω/sqrt(η_i)` (§3.3.2) but doesn't give the general-N
/// combination formula (Vorobyev & Osorio 1998's own closed form is
/// derived specifically for N=3's particular opponent-channel
/// construction). This uses the standard simplifying assumption of
/// independent per-receptor noise, generalizing cleanly to any N:
/// `ΔS = sqrt(Σ_i ((Q'_A,i - Q'_B,i) / e_i)^2)`.
pub fn distance(
    a: &Coordinates,
    b: &Coordinates,
    metric: DistanceMetric,
    noise: Option<&[f64]>,
) -> Option<f64> {
    assert_eq!(
        a.chroma.len(),
        b.chroma.len(),
        "coordinates from different-dimensionality species aren't comparable"
    );
    match metric {
        DistanceMetric::Euclidean => {
            let chroma_sq_sum: f64 = a
                .chroma
                .iter()
                .zip(b.chroma.iter())
                .map(|(x, y)| (x - y).powi(2))
                .sum();
            Some(((a.luminance - b.luminance).powi(2) + chroma_sq_sum).sqrt())
        }
        DistanceMetric::ChromaOnly => {
            let chroma_sq_sum: f64 = a
                .chroma
                .iter()
                .zip(b.chroma.iter())
                .map(|(x, y)| (x - y).powi(2))
                .sum();
            Some(chroma_sq_sum.sqrt())
        }
        DistanceMetric::DeltaS => {
            let noise = noise?;
            if noise.len() != a.adapted_activations.len()
                || noise.len() != b.adapted_activations.len()
            {
                return None;
            }
            let sum: f64 = a
                .adapted_activations
                .iter()
                .zip(b.adapted_activations.iter())
                .zip(noise.iter())
                .map(|((qa, qb), e)| ((qa - qb) / e).powi(2))
                .sum();
            Some(sum.sqrt())
        }
    }
}

/// The symmetric, zero-diagonal N×N matrix of pairwise distances across
/// `coords` (§3.3.2's output format), as `matrix[i][j]`. `None` if
/// `metric` is `DeltaS` and `noise` is unusable (see `distance`).
pub fn difference_matrix(
    coords: &[Coordinates],
    metric: DistanceMetric,
    noise: Option<&[f64]>,
) -> Option<Vec<Vec<f64>>> {
    let n = coords.len();
    let mut matrix = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let d = distance(&coords[i], &coords[j], metric, noise)?;
            matrix[i][j] = d;
            matrix[j][i] = d;
        }
    }
    Some(matrix)
}

/// One species' column in a cross-species ΔS summary (§6.3.2): its own
/// `difference_matrix` under the ΔS metric, plus a data-quality
/// provenance flag (§6.3.2's "literature-grounded vs. approximated"
/// indicator) for display - `None` for `delta_s_matrix` is itself a
/// meaningful, display-worthy state: that species' fixture doesn't have
/// complete per-receptor noise data (§4.2.2), so ΔS can't be computed
/// for it at all, not merely approximated.
pub struct SpeciesDeltaSColumn {
    pub species_name: String,
    pub has_complete_noise_data: bool,
    pub delta_s_matrix: Option<Vec<Vec<f64>>>,
}

/// Computes one `SpeciesDeltaSColumn` per entry in `species`, each
/// evaluating the same `spectra` against the same shared `illuminant`
/// (§6.4's shared-reference-environment constraint - one `illuminant`
/// used for every species' `Pipeline::build` call here) through that
/// species' own independent pipeline (§6.3.1).
pub fn cross_species_delta_s(
    species: &[CurveSet],
    spectra: &[SpectralCurve],
    illuminant: &SpectralCurve,
    step_nm: f64,
) -> Vec<SpeciesDeltaSColumn> {
    species
        .iter()
        .map(|set| {
            let pipeline = Pipeline::build(set.clone(), illuminant, step_nm);
            let noise = pipeline.noise().map(|n| n.to_vec());
            let coords: Vec<Coordinates> =
                spectra.iter().map(|s| pipeline.coordinates(s)).collect();
            let delta_s_matrix =
                difference_matrix(&coords, DistanceMetric::DeltaS, noise.as_deref());
            SpeciesDeltaSColumn {
                species_name: set.name.clone(),
                has_complete_noise_data: noise.is_some(),
                delta_s_matrix,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::CurveType;

    fn coords(luminance: f64, chroma: Vec<f64>) -> Coordinates {
        let saturation = chroma.iter().map(|c| c * c).sum::<f64>().sqrt();
        // adapted_activations isn't exercised by the Euclidean/ChromaOnly
        // tests below; give it a distinct, deliberately-wrong-looking
        // placeholder so a future metric bug (e.g. an accidental fallback
        // to this field for a non-DeltaS metric) would make a hand-
        // calculated assertion fail loudly instead of silently matching.
        Coordinates {
            luminance,
            chroma,
            saturation,
            adapted_activations: vec![f64::NAN],
        }
    }

    #[test]
    fn euclidean_distance_matches_hand_calculation() {
        // L differs by 3, chroma differs by (4,0) -> sqrt(9+16) = 5.
        let a = coords(0.0, vec![0.0, 0.0]);
        let b = coords(3.0, vec![4.0, 0.0]);
        assert!((distance(&a, &b, DistanceMetric::Euclidean, None).unwrap() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn chroma_only_distance_ignores_luminance() {
        // Same chroma difference as above, but luminance difference of
        // 1000 must not affect the chroma-only metric at all.
        let a = coords(0.0, vec![0.0, 0.0]);
        let b = coords(1000.0, vec![4.0, 0.0]);
        assert!((distance(&a, &b, DistanceMetric::ChromaOnly, None).unwrap() - 4.0).abs() < 1e-9);
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn difference_matrix_is_symmetric_with_zero_diagonal() {
        let cs = vec![
            coords(0.0, vec![0.0, 0.0]),
            coords(3.0, vec![4.0, 0.0]),
            coords(0.0, vec![0.0, 5.0]),
        ];
        let m = difference_matrix(&cs, DistanceMetric::Euclidean, None).unwrap();
        for i in 0..3 {
            assert_eq!(m[i][i], 0.0, "diagonal must be zero at {i}");
            for j in 0..3 {
                assert!(
                    (m[i][j] - m[j][i]).abs() < 1e-12,
                    "not symmetric at ({i},{j})"
                );
            }
        }
        // Hand-calculated cross-checks.
        assert!((m[0][1] - 5.0).abs() < 1e-9); // sqrt(9+16)
        assert!((m[0][2] - 5.0).abs() < 1e-9); // sqrt(0+25)
        assert!((m[1][2] - ((9.0 + 16.0 + 25.0_f64).sqrt())).abs() < 1e-9);
    }

    #[test]
    #[should_panic(expected = "different-dimensionality")]
    fn mismatched_dimensionality_panics_rather_than_silently_comparing() {
        let a = coords(0.0, vec![0.0, 0.0]);
        let b = coords(0.0, vec![0.0, 0.0, 0.0]);
        distance(&a, &b, DistanceMetric::Euclidean, None);
    }

    fn coords_with_activations(activations: Vec<f64>) -> Coordinates {
        Coordinates {
            luminance: 0.0,
            chroma: vec![0.0; activations.len().saturating_sub(1)],
            saturation: 0.0,
            adapted_activations: activations,
        }
    }

    #[test]
    fn delta_s_matches_hand_calculation() {
        // Q'_a=[0,0,0], Q'_b=[1,2,3], noise=[1,2,0.5] ->
        // sqrt((1/1)^2 + (2/2)^2 + (3/0.5)^2) = sqrt(1+1+36) = sqrt(38).
        let a = coords_with_activations(vec![0.0, 0.0, 0.0]);
        let b = coords_with_activations(vec![1.0, 2.0, 3.0]);
        let noise = [1.0, 2.0, 0.5];
        let d = distance(&a, &b, DistanceMetric::DeltaS, Some(&noise)).unwrap();
        assert!((d - 38.0_f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn delta_s_is_none_without_noise() {
        let a = coords_with_activations(vec![0.0, 0.0]);
        let b = coords_with_activations(vec![1.0, 1.0]);
        assert_eq!(distance(&a, &b, DistanceMetric::DeltaS, None), None);
    }

    #[test]
    fn delta_s_is_none_when_noise_length_mismatches() {
        let a = coords_with_activations(vec![0.0, 0.0]);
        let b = coords_with_activations(vec![1.0, 1.0]);
        let noise = [1.0, 2.0, 3.0]; // wrong length
        assert_eq!(distance(&a, &b, DistanceMetric::DeltaS, Some(&noise)), None);
    }

    #[test]
    fn difference_matrix_is_none_when_delta_s_unavailable() {
        let cs = vec![
            coords_with_activations(vec![0.0, 0.0]),
            coords_with_activations(vec![1.0, 1.0]),
        ];
        assert_eq!(difference_matrix(&cs, DistanceMetric::DeltaS, None), None);
    }

    fn two_cone_set(name: &str, with_noise: bool) -> CurveSet {
        let mut set = CurveSet::new(name);
        let mut a = SpectralCurve::new("A", CurveType::Sensitivity)
            .with_points(vec![(400.0, 1.0), (500.0, 0.2)]);
        let mut b = SpectralCurve::new("B", CurveType::Sensitivity)
            .with_points(vec![(400.0, 0.2), (500.0, 1.0)]);
        if with_noise {
            a.omega = Some(0.1);
            a.eta = Some(1.0);
            b.omega = Some(0.1);
            b.eta = Some(1.0);
        }
        set.colorspace_curves = vec![a, b];
        set
    }

    #[test]
    fn cross_species_delta_s_flags_missing_noise_data_per_species_independently() {
        // The practical point of this orchestration function (§6.3.1/
        // §6.3.2): one species lacking complete noise data must not
        // block or corrupt the other species' column - each is computed
        // and flagged independently.
        let species = vec![
            two_cone_set("With noise", true),
            two_cone_set("Without noise", false),
        ];
        let illuminant = SpectralCurve::new("flat", CurveType::Illumination)
            .with_points(vec![(400.0, 1.0), (500.0, 1.0)]);
        let spectra = vec![
            SpectralCurve::new("Reddish", CurveType::Reflectance)
                .with_points(vec![(400.0, 0.2), (500.0, 0.8)]),
            SpectralCurve::new("Blueish", CurveType::Reflectance)
                .with_points(vec![(400.0, 0.8), (500.0, 0.2)]),
        ];

        let columns = cross_species_delta_s(&species, &spectra, &illuminant, 10.0);
        assert_eq!(columns.len(), 2);

        assert_eq!(columns[0].species_name, "With noise");
        assert!(columns[0].has_complete_noise_data);
        let matrix0 = columns[0]
            .delta_s_matrix
            .as_ref()
            .expect("has complete noise data, ΔS should be computable");
        assert_eq!(matrix0[0][0], 0.0);
        assert!(
            matrix0[0][1] > 0.0,
            "distinct stimuli should have nonzero ΔS"
        );

        assert_eq!(columns[1].species_name, "Without noise");
        assert!(!columns[1].has_complete_noise_data);
        assert!(
            columns[1].delta_s_matrix.is_none(),
            "ΔS must be unavailable, not silently zero or fabricated"
        );
    }
}
