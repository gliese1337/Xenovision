//! Curve Set data model and JSON persistence (design doc §1.2.2, §1.3).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::curve::SpectralCurve;
use crate::pipeline::generic_candidate_rows;

/// A named, literature-defined opponent contrast (§4.2.4): which curves
/// contribute positively/negatively, and with what weights, to one
/// candidate opponent mechanism (a stored representation of the `Cand_i`
/// construction from §2.2.4). `weights` has one entry per curve in the
/// owning `CurveSet`, in the same order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpponentContrast {
    pub name: String,
    pub weights: Vec<f64>,
}

/// A named collection of curves representing a coherent group - typically
/// one species' full visual system, or one measured object's properties
/// (design doc §1.2.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurveSet {
    pub name: String,
    /// The receptor curves that feed into this set's opponent-process
    /// colorspace (luminance + chroma) - the set the perceptual pipeline
    /// actually operates on.
    pub colorspace_curves: Vec<SpectralCurve>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// Literature-defined opponent contrasts (§4.2.4). Empty means "use
    /// the generic per-receptor candidate formula" (§2.2.4) as a
    /// fallback at calculation time - see `candidate_rows`.
    #[serde(default)]
    pub opponent_contrasts: Vec<OpponentContrast>,
    /// Curves attached to this dataset for completeness but *not* part
    /// of the colorspace `colorspace_curves` set - e.g. a species'
    /// photoreceptor classes that are documented but don't participate
    /// in color-opponent perception (§2.3.9's swallowtail butterfly: a
    /// UV-filtered violet receptor and a motion-associated broad-band
    /// receptor, alongside its 4 color-vision receptors). Generic rather
    /// than species-specific: any `CurveSet` can have curves here, with
    /// no code path that assumes which species does.
    #[serde(default)]
    pub isolated_curves: Vec<SpectralCurve>,
}

#[derive(Debug, thiserror::Error)]
pub enum CurveSetError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path} as a Curve Set: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

impl CurveSet {
    pub fn new(name: impl Into<String>) -> Self {
        CurveSet {
            name: name.into(),
            colorspace_curves: Vec::new(),
            metadata: BTreeMap::new(),
            opponent_contrasts: Vec::new(),
            isolated_curves: Vec::new(),
        }
    }

    /// The candidate opponent-contrast rows to use for this set's chroma
    /// construction (§2.2.4): the stored literature contrasts if any are
    /// defined, else the generic per-receptor "candidate vs. mean of
    /// others" fallback (§4.2.4).
    pub fn candidate_rows(&self) -> Vec<Vec<f64>> {
        if self.opponent_contrasts.is_empty() {
            generic_candidate_rows(self.colorspace_curves.len())
        } else {
            self.opponent_contrasts
                .iter()
                .map(|c| c.weights.clone())
                .collect()
        }
    }

    /// Effective luminance weight for each curve, in order (§4.2.3): the
    /// curve's explicit `luminance_weight` override if set, else its own
    /// integral normalized against the sum of every curve's integral in
    /// this set.
    pub fn luminance_weights(&self, step_nm: f64) -> Vec<f64> {
        let integrals: Vec<f64> = self
            .colorspace_curves
            .iter()
            .map(|c| c.integral(step_nm))
            .collect();
        let total: f64 = integrals.iter().sum();
        self.colorspace_curves
            .iter()
            .zip(integrals.iter())
            .map(|(c, &integral)| {
                c.luminance_weight
                    .unwrap_or(if total > 0.0 { integral / total } else { 0.0 })
            })
            .collect()
    }

    /// `true` if some but not all curves in this set have `eta`
    /// (relative receptor density) defined - the specific "incomplete/
    /// unreliable noise model" case §4.2.2 says should be flagged to the
    /// user (having it on *all* curves, or *none*, is fine).
    pub fn has_partial_eta_coverage(&self) -> bool {
        let defined = self
            .colorspace_curves
            .iter()
            .filter(|c| c.eta.is_some())
            .count();
        defined != 0 && defined != self.colorspace_curves.len()
    }

    /// Per-receptor noise terms for the Vorobyev-Osorio ΔS metric
    /// (§3.3.2): `e_i = omega_i / sqrt(eta_i_normalized)`, with `eta`
    /// normalized by the sum of `eta` across every curve. `None` if any
    /// curve is missing `omega` or `eta` - ΔS needs noise data for every
    /// receptor to be meaningful (the softer "some but not all" warning
    /// is `has_partial_eta_coverage`, tracked separately since partial
    /// coverage with the rest left blank is still a valid, if flagged,
    /// state - unlike *fully* missing data, which blocks the metric).
    pub fn receptor_noise(&self) -> Option<Vec<f64>> {
        let omegas: Vec<f64> = self
            .colorspace_curves
            .iter()
            .map(|c| c.omega)
            .collect::<Option<Vec<_>>>()?;
        let etas: Vec<f64> = self
            .colorspace_curves
            .iter()
            .map(|c| c.eta)
            .collect::<Option<Vec<_>>>()?;
        let eta_sum: f64 = etas.iter().sum();
        if eta_sum <= 0.0 {
            return None;
        }
        Some(
            omegas
                .iter()
                .zip(etas.iter())
                .map(|(&omega, &eta)| omega / (eta / eta_sum).sqrt())
                .collect(),
        )
    }

    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<(), CurveSetError> {
        let path = path.as_ref();
        let json = serde_json::to_string_pretty(self).expect("CurveSet is always serializable");
        std::fs::write(path, json).map_err(|source| CurveSetError::Io {
            path: path.display().to_string(),
            source,
        })
    }

    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, CurveSetError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| CurveSetError::Io {
            path: path.display().to_string(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|source| CurveSetError::Parse {
            path: path.display().to_string(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::CurveType;

    fn sample_set() -> CurveSet {
        let mut set = CurveSet::new("Human (Homo sapiens)");
        set.colorspace_curves.push(
            SpectralCurve::new("S-cone", CurveType::Sensitivity).with_points(vec![
                (380.0, 0.0),
                (420.0, 1.0),
                (480.0, 0.1),
            ]),
        );
        set.colorspace_curves.push(
            SpectralCurve::new("M-cone", CurveType::Sensitivity).with_points(vec![
                (480.0, 0.2),
                (530.0, 1.0),
                (580.0, 0.3),
            ]),
        );
        set.metadata
            .insert("citation".to_string(), "Schnapf et al. 1987".to_string());
        set
    }

    #[test]
    fn candidate_rows_falls_back_to_generic_when_no_opponent_contrasts_stored() {
        let set = sample_set();
        assert_eq!(set.candidate_rows(), generic_candidate_rows(2));
    }

    #[test]
    fn candidate_rows_uses_stored_opponent_contrasts_when_present() {
        let mut set = sample_set();
        set.opponent_contrasts.push(OpponentContrast {
            name: "S - M".to_string(),
            weights: vec![1.0, -1.0],
        });
        assert_eq!(set.candidate_rows(), vec![vec![1.0, -1.0]]);
    }

    #[test]
    fn luminance_weights_use_integral_default_when_unset() {
        let set = sample_set();
        let weights = set.luminance_weights(1.0);
        let integrals: Vec<f64> = set
            .colorspace_curves
            .iter()
            .map(|c| c.integral(1.0))
            .collect();
        let total: f64 = integrals.iter().sum();
        for (w, integral) in weights.iter().zip(integrals.iter()) {
            assert!((w - integral / total).abs() < 1e-9);
        }
        // Both curves have positive integral, so weights should sum to 1.
        assert!((weights.iter().sum::<f64>() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn luminance_weights_respect_explicit_override() {
        let mut set = sample_set();
        set.colorspace_curves[0].luminance_weight = Some(0.9);
        let weights = set.luminance_weights(1.0);
        assert_eq!(weights[0], 0.9);
        // The other curve still gets its integral-derived default, not
        // renormalized against the override - explicit values are used
        // verbatim (§4.2.3), not folded back into a sum-to-1 partition.
        let m_integral = set.colorspace_curves[1].integral(1.0);
        let total: f64 = set.colorspace_curves.iter().map(|c| c.integral(1.0)).sum();
        assert!((weights[1] - m_integral / total).abs() < 1e-9);
    }

    #[test]
    fn eta_coverage_flag() {
        let mut set = sample_set();
        assert!(!set.has_partial_eta_coverage(), "none set -> not partial");
        set.colorspace_curves[0].eta = Some(1.0);
        assert!(set.has_partial_eta_coverage(), "one of two set -> partial");
        set.colorspace_curves[1].eta = Some(2.0);
        assert!(!set.has_partial_eta_coverage(), "both set -> not partial");
    }

    #[test]
    fn receptor_noise_requires_complete_data() {
        let mut set = sample_set();
        assert_eq!(set.receptor_noise(), None, "no omega/eta at all");
        set.colorspace_curves[0].omega = Some(0.05);
        set.colorspace_curves[0].eta = Some(1.0);
        assert_eq!(set.receptor_noise(), None, "only one curve has data");
    }

    #[test]
    fn receptor_noise_hand_calculation() {
        let mut set = sample_set();
        // omega=0.1 for both; eta = 1 and 3 (normalized: 0.25 and 0.75).
        set.colorspace_curves[0].omega = Some(0.1);
        set.colorspace_curves[0].eta = Some(1.0);
        set.colorspace_curves[1].omega = Some(0.1);
        set.colorspace_curves[1].eta = Some(3.0);
        let noise = set.receptor_noise().unwrap();
        assert!((noise[0] - 0.1 / 0.25_f64.sqrt()).abs() < 1e-9);
        assert!((noise[1] - 0.1 / 0.75_f64.sqrt()).abs() < 1e-9);
        // Higher relative density (eta) -> lower noise, per the design doc.
        assert!(noise[1] < noise[0]);
    }

    #[test]
    fn round_trip_json_in_memory() {
        let set = sample_set();
        let json = serde_json::to_string(&set).unwrap();
        let back: CurveSet = serde_json::from_str(&json).unwrap();
        assert_eq!(set, back);
    }

    #[test]
    fn save_and_load_round_trip_through_disk() {
        let set = sample_set();
        let dir = std::env::temp_dir().join(format!("xenovision-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("human.json");

        set.save_to_file(&path).unwrap();
        let loaded = CurveSet::load_from_file(&path).unwrap();
        assert_eq!(set, loaded);

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn load_missing_file_is_an_io_error() {
        let err = CurveSet::load_from_file("/nonexistent/path/does-not-exist.json").unwrap_err();
        assert!(matches!(err, CurveSetError::Io { .. }));
    }

    #[test]
    fn load_malformed_json_is_a_parse_error() {
        let dir = std::env::temp_dir().join(format!("xenovision-test-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.json");
        std::fs::write(&path, "{ not valid json").unwrap();

        let err = CurveSet::load_from_file(&path).unwrap_err();
        assert!(matches!(err, CurveSetError::Parse { .. }));

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }
}
