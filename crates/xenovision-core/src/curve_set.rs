//! Curve Set data model and JSON persistence (design doc §1.2.2, §1.3).
//!
//! A `CurveSet` holds one *receptor pool* (`curves`) plus a list of
//! `regimes`. A regime defines a colorspace: which receptors belong to
//! it, in what order, plus optional per-regime luminance-weight and
//! opponent-contrast overrides. A receptor belongs to a colorspace iff
//! it is a member of at least one regime; a receptor in no regime is
//! reported individually (activation/operating state) but never
//! contributes to any luminance/chroma calculation.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};

use crate::curve::{DEFAULT_ILLUMINATION_SOFTNESS, SpectralCurve};
use crate::pipeline::generic_candidate_rows;

/// A named, literature-defined opponent contrast (§4.2.4): which curves
/// contribute positively/negatively, and with what weights, to one
/// candidate opponent mechanism (a stored representation of the `Cand_i`
/// construction from §2.2.4). `weights` has one entry per member curve
/// of the regime that owns it, in that regime's member order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpponentContrast {
    pub name: String,
    pub weights: Vec<f64>,
}

/// One colorspace regime (§2.2): a coherent group of the set's
/// receptors that jointly produce a luminance + chroma colorspace, with
/// its own receptor membership, luminance weighting, and opponent-
/// contrast rows. Regimes are structurally distinct colorspaces - not
/// gated copies of one master colorspace. A species may declare several
/// (e.g. the frog's scotopic/mesopic/photopic: §2.3.10), or none at all
/// (§10.2's Mantis Shrimp, whose receptors never enter any colorspace).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Regime {
    pub name: String,
    /// Indices into the owning `CurveSet.curves` (pool order) of the
    /// receptors that make up this colorspace, in construction order.
    /// A curve may appear in several regimes (e.g. the frog's rods are
    /// in both its scotopic and mesopic regimes).
    pub curves: Vec<u64>,
    /// Per-regime luminance-weight overrides, parallel to `curves`.
    /// `None` = each member's curve-level `luminance_weight` override or
    /// the integral-derived default.
    #[serde(default)]
    pub luminance_weights: Option<Vec<f64>>,
    /// Per-regime literature-defined opponent contrasts, weights
    /// parallel to `curves`. `None` (or an empty, clear-all list) = the
    /// generic per-receptor fallback (§2.2.4).
    #[serde(default)]
    pub opponent_contrasts: Option<Vec<OpponentContrast>>,
}

impl Regime {
    /// Builds a new regime with plain (no-override) config.
    pub fn new(name: impl Into<String>, curves: Vec<u64>) -> Self {
        Regime {
            name: name.into(),
            curves,
            luminance_weights: None,
            opponent_contrasts: None,
        }
    }

    /// A regime whose membership and overrides are all empty - the
    /// default value for a freshly-created regime.
    pub fn empty(name: impl Into<String>) -> Self {
        Regime::new(name, Vec::new())
    }
}

/// A resolved, pipeline-ready view of one regime (see
/// `CurveSet::regime_config`): the member curves plus every derived
/// quantity the pipeline needs, with the regime's overrides already
/// applied and the set-level operating-window defaults carried along for
/// the members' own windows to fall back to.
#[derive(Debug, Clone, PartialEq)]
pub struct RegimeConfig {
    pub name: String,
    /// The regime's member curves, in member order.
    pub curves: Vec<SpectralCurve>,
    /// Effective luminance weights, in member order: the regime's
    /// explicit override if set, else each member's curve-level override
    /// or its integral-derived share.
    pub luminance_weights: Vec<f64>,
    /// Effective candidate opponent-contrast rows, in member order: the
    /// regime's literature contrasts if set, else the generic
    /// per-receptor fallback.
    pub candidate_rows: Vec<Vec<f64>>,
    /// Per-member Vorobyev-Osorio noise terms, if every member has
    /// complete omega/eta data (`None` otherwise).
    pub noise: Option<Vec<f64>>,
    /// The owning set's operating-window defaults, which members without
    /// their own bounds fall back to.
    pub illumination_min: Option<f64>,
    pub illumination_max: Option<f64>,
    pub illumination_softness: Option<f64>,
}

/// A named collection of curves representing a coherent group - typically
/// one species' full visual system, or one measured object's properties
/// (design doc §1.2.2). `curves` is the receptor pool; colorspace
/// membership lives entirely in `regimes`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CurveSet {
    pub name: String,
    /// The receptor pool: every photoreceptor class of this system, in
    /// order. A receptor belongs to a colorspace iff some regime lists
    /// its index; receptors in no regime are what the model's old
    /// separate `isolated_curves` list used to hold.
    pub curves: Vec<SpectralCurve>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// The set's colorspace regimes. Empty = the species has no
    /// colorspace at all (every receptor reported individually - e.g.
    /// Mantis Shrimp, §10.2). Legacy files that used a single implicit
    /// colorspace list (`colorspace_curves`) load with one anonymous
    /// regime synthesized over it.
    #[serde(default)]
    pub regimes: Vec<Regime>,

    /// Set-level default lower bound for receptor operating windows
    /// (§2.2) - applies to any curve whose own `illumination_min` is
    /// unset. Units: ambient irradiance W/m²
    /// (`illumination::irradiance_w_m2` convention). `None` means no
    /// lower bound. Lets whole regime-specific systems (e.g. the frog's
    /// scotopic or photopic regimes) be characterized once instead of on
    /// every receptor.
    #[serde(default)]
    pub illumination_min: Option<f64>,
    /// Set-level default upper bound for receptor operating windows, same
    /// fallback semantics as `illumination_min`. Units: W/m².
    #[serde(default)]
    pub illumination_max: Option<f64>,
    /// Set-level default shoulder width (decades) for receptor operating
    /// windows, used when a curve sets a bound but no explicit softness.
    #[serde(default)]
    pub illumination_softness: Option<f64>,
}

/// Legacy on-disk format (pre-regime): `colorspace_curves` +
/// `isolated_curves` + a set-level `opponent_contrasts` list. Migrated
/// on load into a single receptor pool and one anonymous regime over the
/// former colorspace curves. Field names are the raw snake_case serde
/// defaults (the legacy writer used the plain Rust field names).
#[derive(Deserialize)]
struct LegacyCurveSet {
    name: String,
    colorspace_curves: Option<Vec<SpectralCurve>>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    #[serde(default)]
    opponent_contrasts: Vec<OpponentContrast>,
    #[serde(default)]
    isolated_curves: Vec<SpectralCurve>,
    #[serde(default)]
    illumination_min: Option<f64>,
    #[serde(default)]
    illumination_max: Option<f64>,
    #[serde(default)]
    illumination_softness: Option<f64>,
}

/// Current on-disk format: one `curves` pool + explicit `regimes`.
#[derive(Deserialize)]
struct CurrentCurveSet {
    name: String,
    curves: Vec<SpectralCurve>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    #[serde(default)]
    regimes: Vec<Regime>,
    #[serde(default)]
    illumination_min: Option<f64>,
    #[serde(default)]
    illumination_max: Option<f64>,
    #[serde(default)]
    illumination_softness: Option<f64>,
}

/// Name given to the anonymous single regime synthesized when a legacy
/// file (which had one implicit colorspace list) loads.
pub const LEGACY_ANONYMOUS_REGIME_NAME: &str = "Colorspace";

impl<'de> Deserialize<'de> for CurveSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Both formats are tried by shape. `curves` being present
        // identifies the current format even when `regimes` is empty
        // (a genuine "no colorspace" species like mantis shrimp): we must
        // not migrate a deliberately-empty regime list into an anonymous
        // regime. The legacy format is recognized by its
        // `colorspace_curves` list.
        let value = serde_json::Value::deserialize(deserializer)?;
        let has_curves = value.get("curves").map(serde_json::Value::is_array).unwrap_or(false);
        let parse = |value: serde_json::Value| -> Result<CurveSet, D::Error> {
            if has_curves {
                let current: CurrentCurveSet =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                Ok(CurveSet {
                    name: current.name,
                    curves: current.curves,
                    metadata: current.metadata,
                    regimes: current.regimes,
                    illumination_min: current.illumination_min,
                    illumination_max: current.illumination_max,
                    illumination_softness: current.illumination_softness,
                })
            } else {
                let legacy: LegacyCurveSet =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                let mut curves = legacy.colorspace_curves.unwrap_or_default();
                let colorspace_len = curves.len();
                curves.extend(legacy.isolated_curves);
                let regimes = if colorspace_len > 0 {
                    let contrasts = legacy.opponent_contrasts;
                    vec![Regime {
                        name: LEGACY_ANONYMOUS_REGIME_NAME.to_string(),
                        curves: (0..colorspace_len as u64).collect(),
                        luminance_weights: None,
                        opponent_contrasts: if contrasts.is_empty() {
                            None
                        } else {
                            Some(contrasts)
                        },
                    }]
                } else {
                    Vec::new()
                };
                Ok(CurveSet {
                    name: legacy.name,
                    curves,
                    metadata: legacy.metadata,
                    regimes,
                    illumination_min: legacy.illumination_min,
                    illumination_max: legacy.illumination_max,
                    illumination_softness: legacy.illumination_softness,
                })
            }
        };
        parse(value)
    }
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
            curves: Vec::new(),
            metadata: BTreeMap::new(),
            regimes: Vec::new(),
            illumination_min: None,
            illumination_max: None,
            illumination_softness: None,
        }
    }

    /// Effective operating window for one curve: its own bound if set,
    /// else the set-level default.
    fn effective_illumination_window(
        &self,
        curve: &SpectralCurve,
    ) -> (Option<f64>, Option<f64>) {
        (
            curve.illumination_min.or(self.illumination_min),
            curve.illumination_max.or(self.illumination_max),
        )
    }

    /// The effective softness for one curve: its own if set, else the
    /// set-level default, else `DEFAULT_ILLUMINATION_SOFTNESS`.
    fn effective_illumination_softness(&self, curve: &SpectralCurve) -> f64 {
        curve
            .illumination_softness
            .or(self.illumination_softness)
            .unwrap_or(DEFAULT_ILLUMINATION_SOFTNESS)
    }

    /// Per-receptor operating gains `g_i` (§2.2) for `curves`, in order,
    /// from the irradiance **each receptor actually sees**
    /// (`seen_irradiances[i] = ∫ S_i(λ)·I(λ) dλ`, in W/m² under the
    /// convention that luminant curves are spectral irradiance in
    /// W/m²/nm and receptor sensitivities are dimensionless weights).
    /// Gating on the receptor-filtered ambient light means e.g. a
    /// short-wavelength receptor is not gated on just because the room
    /// is bright at long wavelengths - the light that matters is the
    /// light that receptor absorbs. `seen_irradiances` must parallel
    /// `curves`; the caller owns the integration
    /// (`pipeline::integrate_product`). A curve with no effective bounds
    /// gets gain 1.0 (always on), preserving the pre-existing behavior.
    fn gating_gains_for(&self, curves: &[SpectralCurve], seen_irradiances: &[f64]) -> Vec<f64> {
        curves
            .iter()
            .zip(seen_irradiances)
            .map(|(c, &seen)| {
                let (lo, hi) = self.effective_illumination_window(c);
                crate::curve::operating_gain(seen, lo, hi, self.effective_illumination_softness(c))
            })
            .collect()
    }

    /// Operating gains for every receptor in the pool, in pool order
    /// (§2.2), from the same receptor-filtered seen irradiances used by
    /// the regimes' own pipelines. Receptors in no regime never enter an
    /// opponent pipeline, but their individual activations (§10.1) are
    /// still reported, so their own operating windows are still honored.
    pub fn receptor_gains(&self, seen_irradiances: &[f64]) -> Vec<f64> {
        self.gating_gains_for(&self.curves, seen_irradiances)
    }

    /// `true` if any receptor in the pool (or the set-level defaults)
    /// declares a malformed illumination window - a non-positive bound,
    /// or `min >= max`. Used to surface the mistake in the UI;
    /// calculation treats such a window as unbounded.
    pub fn has_invalid_illumination_window(&self) -> bool {
        let set_crossed = matches!(
            (self.illumination_min, self.illumination_max),
            (Some(lo), Some(hi)) if lo >= hi
        );
        set_crossed
            || [self.illumination_min, self.illumination_max]
                .into_iter()
                .flatten()
                .any(|b| b <= 0.0)
            || self.curves.iter().any(|c| {
                c.has_invalid_illumination_window()
                    || {
                        // Per-curve bounds can also cross the set
                        // default they fall back to.
                        let (lo, hi) = self.effective_illumination_window(c);
                        matches!((lo, hi), (Some(lo), Some(hi)) if lo >= hi)
                    }
            })
    }

    /// Resolves one regime into the pipeline-ready `RegimeConfig`.
    /// Indices are validated against the pool (a hand-edited file may
    /// reference a curve that no longer exists); members whose index is
    /// out of range are skipped, and any per-regime override whose length
    /// no longer matches the resolved membership is ignored.
    pub fn regime_config(&self, regime: &Regime) -> RegimeConfig {
        let member_indices: Vec<u64> = regime
            .curves
            .iter()
            .filter(|&&i| (i as usize) < self.curves.len())
            .copied()
            .collect();
        let curves: Vec<SpectralCurve> = member_indices
            .iter()
            .map(|&i| self.curves[i as usize].clone())
            .collect();

        let luminance_weights = regime
            .luminance_weights
            .as_ref()
            .filter(|w| w.len() == curves.len())
            .cloned()
            .unwrap_or_else(|| luminance_weights_for(&curves));

        let candidate_rows = regime
            .opponent_contrasts
            .as_ref()
            .filter(|oc| !oc.is_empty() && oc.iter().all(|c| c.weights.len() == curves.len()))
            .map(|ocs| ocs.iter().map(|c| c.weights.clone()).collect())
            .unwrap_or_else(|| generic_candidate_rows(curves.len()));

        let noise = receptor_noise_for(&curves);

        RegimeConfig {
            name: regime.name.clone(),
            curves,
            luminance_weights,
            candidate_rows,
            noise,
            illumination_min: self.illumination_min,
            illumination_max: self.illumination_max,
            illumination_softness: self.illumination_softness,
        }
    }

    /// `regime_config` for the regime at `index`, or `None` if out of
    /// range (including a species with no declared regimes at all).
    pub fn regime_config_by_index(&self, index: usize) -> Option<RegimeConfig> {
        self.regimes.get(index).map(|r| self.regime_config(r))
    }

    /// The resolved config for the set's first regime - or, for a set
    /// with no regimes, an empty zero-member config named after the set
    /// itself (the N=0 degenerate colorspace). This is the plain "one
    /// colorspace" reading of a set, for the parts of the codebase that
    /// predate multi-regime (generic pipeline tests, the ΔS orchestration
    /// helper). Callers that need to honor the "no colorspace at all"
    /// semantics should check `regimes.is_empty()` first (the app does,
    /// to report a regime-less species' receptors individually instead).
    pub fn default_regime_config(&self) -> RegimeConfig {
        match self.regimes.first() {
            Some(r) => self.regime_config(r),
            None => RegimeConfig {
                name: self.name.clone(),
                curves: Vec::new(),
                luminance_weights: Vec::new(),
                candidate_rows: Vec::new(),
                noise: None,
                illumination_min: self.illumination_min,
                illumination_max: self.illumination_max,
                illumination_softness: self.illumination_softness,
            },
        }
    }

    /// `true` if some but not all receptors in the pool have `eta`
    /// (relative receptor density) defined - the specific "incomplete/
    /// unreliable noise model" case §4.2.2 says should be flagged to the
    /// user (having it on *all* curves, or *none*, is fine).
    pub fn has_partial_eta_coverage(&self) -> bool {
        let defined = self.curves.iter().filter(|c| c.eta.is_some()).count();
        defined != 0 && defined != self.curves.len()
    }

    /// Per-receptor noise terms for the Vorobyev-Osorio ΔS metric
    /// (§3.3.2) across the whole pool: `e_i = omega_i / sqrt(eta_i_normalized)`,
    /// with `eta` normalized by the sum of `eta` across every receptor.
    /// `None` if any receptor is missing `omega` or `eta` - ΔS needs noise
    /// data for every receptor to be meaningful. (Per-regime noise,
    /// restricted to a regime's members, is exposed through
    /// `regime_config`'s `RegimeConfig.noise`.)
    pub fn receptor_noise(&self) -> Option<Vec<f64>> {
        receptor_noise_for(&self.curves)
    }

    /// Integral-derived luminance weights across the whole pool, in pool
    /// order (§4.2.3): each receptor's explicit `luminance_weight`
    /// override if set, else its own integral normalized against the sum
    /// of every receptor's integral. The `step_nm` only scales every
    /// integral equally, so it cancels in the normalization - kept for
    /// API compatibility with the previous colorspace-scoped version.
    pub fn luminance_weights(&self, _step_nm: f64) -> Vec<f64> {
        luminance_weights_for(&self.curves)
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

/// Effective luminance weight for each curve in `curves`, in order
/// (§4.2.3): the curve's explicit `luminance_weight` override if set,
/// else its own integral normalized against the sum of every curve's
/// integral.
pub fn luminance_weights_for(curves: &[SpectralCurve]) -> Vec<f64> {
    let integrals: Vec<f64> = curves.iter().map(|c| c.integral(1.0)).collect();
    let total: f64 = integrals.iter().sum();
    curves
        .iter()
        .zip(integrals.iter())
        .map(|(c, &integral)| {
            c.luminance_weight
                .unwrap_or(if total > 0.0 { integral / total } else { 0.0 })
        })
        .collect()
}

/// Per-receptor noise terms (`§3.3.2`) for `curves`, or `None` if any
/// curve lacks `omega` or `eta`, or the etas sum to zero.
pub fn receptor_noise_for(curves: &[SpectralCurve]) -> Option<Vec<f64>> {
    let omegas: Vec<f64> = curves.iter().map(|c| c.omega).collect::<Option<Vec<_>>>()?;
    let etas: Vec<f64> = curves.iter().map(|c| c.eta).collect::<Option<Vec<_>>>()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::CurveType;

    fn sample_set() -> CurveSet {
        let mut set = CurveSet::new("Human (Homo sapiens)");
        set.curves.push(
            SpectralCurve::new("S-cone", CurveType::Sensitivity).with_points(vec![
                (380.0, 0.0),
                (420.0, 1.0),
                (480.0, 0.1),
            ]),
        );
        set.curves.push(
            SpectralCurve::new("M-cone", CurveType::Sensitivity).with_points(vec![
                (480.0, 0.2),
                (530.0, 1.0),
                (580.0, 0.3),
            ]),
        );
        set.regimes.push(Regime::new("Trichromat", vec![0, 1]));
        set.metadata
            .insert("citation".to_string(), "Schnapf et al. 1987".to_string());
        set
    }

    fn singleton_pool(name: &str) -> CurveSet {
        let mut set = CurveSet::new(name);
        set.curves.push(
            SpectralCurve::new("only", CurveType::Sensitivity)
                .with_points(vec![(400.0, 1.0), (500.0, 0.4)]),
        );
        set
    }

    #[test]
    fn candidate_rows_falls_back_to_generic_when_no_regime_contrasts_stored() {
        let set = sample_set();
        let config = set.regime_config(&set.regimes[0]);
        assert_eq!(config.candidate_rows, generic_candidate_rows(2));
    }

    #[test]
    fn candidate_rows_uses_regime_contrasts_when_present() {
        let mut set = sample_set();
        set.regimes[0].opponent_contrasts = Some(vec![OpponentContrast {
            name: "S - M".to_string(),
            weights: vec![1.0, -1.0],
        }]);
        let config = set.regime_config(&set.regimes[0]);
        assert_eq!(config.candidate_rows, vec![vec![1.0, -1.0]]);
    }

    #[test]
    fn luminance_weights_use_integral_default_when_unset() {
        let set = sample_set();
        let weights = set.luminance_weights(1.0);
        let integrals: Vec<f64> = set
            .curves
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
        set.curves[0].luminance_weight = Some(0.9);
        let weights = set.luminance_weights(1.0);
        assert_eq!(weights[0], 0.9);
        // The other curve still gets its integral-derived default, not
        // renormalized against the override - explicit values are used
        // verbatim (§4.2.3), not folded back into a sum-to-1 partition.
        let m_integral = set.curves[1].integral(1.0);
        let total: f64 = set.curves.iter().map(|c| c.integral(1.0)).sum();
        assert!((weights[1] - m_integral / total).abs() < 1e-9);
    }

    #[test]
    fn regime_luminance_override_wins_over_curve_level_defaults() {
        let mut set = sample_set();
        set.curves[0].luminance_weight = Some(0.7); // curve-level override
        set.regimes[0].luminance_weights = Some(vec![0.0, 0.0]); // regime override
        let config = set.regime_config(&set.regimes[0]);
        assert_eq!(config.luminance_weights, vec![0.0, 0.0]);
    }

    #[test]
    fn regime_config_skips_out_of_range_members_and_ignores_stale_overrides() {
        let mut set = singleton_pool("broken");
        // Membership references curve 9 (out of range) and curve 0.
        set.regimes.push(Regime {
            name: "Broken".to_string(),
            curves: vec![9, 0],
            luminance_weights: Some(vec![1.0, 1.0, 1.0]), // stale length
            opponent_contrasts: Some(vec![OpponentContrast {
                name: "stale".to_string(),
                weights: vec![1.0],
            }]),
        });
        let config = set.regime_config(&set.regimes[0]);
        assert_eq!(config.curves.len(), 1);
        assert_eq!(config.candidate_rows, generic_candidate_rows(1));
    }

    #[test]
    fn eta_coverage_flag() {
        let mut set = sample_set();
        assert!(!set.has_partial_eta_coverage(), "none set -> not partial");
        set.curves[0].eta = Some(1.0);
        assert!(set.has_partial_eta_coverage(), "one of two set -> partial");
        set.curves[1].eta = Some(2.0);
        assert!(!set.has_partial_eta_coverage(), "both set -> not partial");
    }

    #[test]
    fn receptor_noise_requires_complete_data() {
        let mut set = sample_set();
        assert_eq!(set.receptor_noise(), None, "no omega/eta at all");
        set.curves[0].omega = Some(0.05);
        set.curves[0].eta = Some(1.0);
        assert_eq!(set.receptor_noise(), None, "only one curve has data");
    }

    #[test]
    fn receptor_noise_hand_calculation() {
        let mut set = sample_set();
        // omega=0.1 for both; eta = 1 and 3 (normalized: 0.25 and 0.75).
        set.curves[0].omega = Some(0.1);
        set.curves[0].eta = Some(1.0);
        set.curves[1].omega = Some(0.1);
        set.curves[1].eta = Some(3.0);
        let noise = set.receptor_noise().unwrap();
        assert!((noise[0] - 0.1 / 0.25_f64.sqrt()).abs() < 1e-9);
        assert!((noise[1] - 0.1 / 0.75_f64.sqrt()).abs() < 1e-9);
        // Higher relative density (eta) -> lower noise, per the design doc.
        assert!(noise[1] < noise[0]);
    }

    #[test]
    fn regime_noise_is_scoped_to_the_regimes_members() {
        let mut set = CurveSet::new("Two-regime set");
        let mut rod = SpectralCurve::new("rod", CurveType::Sensitivity)
            .with_points(vec![(400.0, 1.0), (500.0, 0.4)]);
        rod.omega = Some(0.1);
        rod.eta = Some(1.0);
        // Cone has no noise data; scotopic-only regime (rod) still has it.
        let mut cone = SpectralCurve::new("cone", CurveType::Sensitivity)
            .with_points(vec![(400.0, 0.4), (500.0, 1.0)]);
        cone.omega = Some(0.1);
        cone.eta = Some(3.0);
        set.curves = vec![rod, cone.clone()];
        set.regimes = vec![Regime::new("Scotopic", vec![0]), Regime::new("Mesopic", vec![0, 1])];

        let scotopic = set.regime_config(&set.regimes[0]);
        assert_eq!(scotopic.noise.unwrap().len(), 1);
        // Mesopic needs data on both members - if the cone's were missing,
        // noise would be None.
        let mut cone_blank = cone;
        cone_blank.omega = None;
        cone_blank.eta = None;
        set.curves[1] = cone_blank;
        let mesopic = set.regime_config(&set.regimes[1]);
        assert_eq!(mesopic.noise, None);
        // Pool-wide noise is None too (partial coverage).
        assert_eq!(set.receptor_noise(), None);
    }

    #[test]
    fn round_trip_json_in_memory() {
        let set = sample_set();
        let json = serde_json::to_string(&set).unwrap();
        let back: CurveSet = serde_json::from_str(&json).unwrap();
        assert_eq!(set, back);
    }

    #[test]
    fn empty_regimes_list_round_trips_as_no_colorspace() {
        // A current-format file with `regimes` explicitly empty must load
        // back with no regimes - the mantis-shrimp "no colorspace"
        // species must not gain an anonymous regime.
        let mut set = CurveSet::new("No colorspace");
        set.curves.push(
            SpectralCurve::new("R1", CurveType::Sensitivity)
                .with_points(vec![(400.0, 1.0), (500.0, 0.4)]),
        );
        let json = serde_json::to_string(&set).unwrap();
        let back: CurveSet = serde_json::from_str(&json).unwrap();
        assert!(back.regimes.is_empty());
        assert_eq!(back.curves.len(), 1);
    }

    #[test]
    fn legacy_colorspace_isolated_lists_migrate_to_pool_plus_anonymous_regime() {
        // A pre-regime file: colorspace + isolated lists, plus a
        // set-level opponent contrast. Loading must produce one pool
        // (colorspace first, then isolated) and one anonymous regime
        // carrying the legacy contrasts, leaving the former isolated
        // curves unaffiliated.
        let json = r#"{
            "name": "Legacy species",
            "colorspace_curves": [
                {"name": "A", "curve_type": "sensitivity",
                 "points": [[400.0, 1.0]]}
            ],
            "metadata": {},
            "opponent_contrasts": [
                {"name": "A vs mean", "weights": [2.0]}
            ],
            "isolated_curves": [
                {"name": "B", "curve_type": "sensitivity",
                 "points": [[500.0, 1.0]]}
            ],
            "illumination_min": 0.01
        }"#;
        let set: CurveSet = serde_json::from_str(json).unwrap();
        assert_eq!(set.curves.len(), 2);
        assert_eq!(set.curves[0].name, "A");
        assert_eq!(set.curves[1].name, "B");
        assert_eq!(set.regimes.len(), 1);
        assert_eq!(set.regimes[0].name, LEGACY_ANONYMOUS_REGIME_NAME);
        assert_eq!(set.regimes[0].curves, vec![0]);
        assert_eq!(
            set.regimes[0].opponent_contrasts.as_ref().unwrap()[0].weights,
            vec![2.0]
        );
        assert_eq!(set.illumination_min, Some(0.01));
        // The anonymous regime's pipeline-ready config matches the old
        // colorspace-list behavior.
        let config = set.regime_config(&set.regimes[0]);
        assert_eq!(config.luminance_weights.len(), 1);
        assert_eq!(config.candidate_rows, vec![vec![2.0]]);
    }

    #[test]
    fn legacy_file_with_empty_colorspace_list_loads_with_no_regimes() {
        // A legacy mantis-shaped file: everything isolated, empty
        // colorspace. Must load with a 1-curve pool and no regimes - the
        // "no colorspace" reading, not an anonymous empty regime.
        let json = r#"{
            "name": "Legacy all-isolated",
            "colorspace_curves": [],
            "isolated_curves": [
                {"name": "R", "curve_type": "sensitivity",
                 "points": [[400.0, 1.0]]}
            ]
        }"#;
        let set: CurveSet = serde_json::from_str(json).unwrap();
        assert_eq!(set.curves.len(), 1);
        assert!(set.regimes.is_empty());
    }

    #[test]
    fn regime_gain_matches_product_of_member_gains_for_disjoint_windows() {
        // Rod window at dim levels, cone at bright ones - the product
        // over both members is only near 1 where both windows contain the
        // seen irradiance.
        let mut set = CurveSet::new("Regime pair");
        set.curves = vec![
            SpectralCurve::new("Dim", CurveType::Sensitivity)
                .with_points(vec![(400.0, 1.0), (500.0, 0.4)])
                .with_illumination_window(Some(1e-3), Some(1e-1)),
            SpectralCurve::new("Bright", CurveType::Sensitivity)
                .with_points(vec![(400.0, 0.4), (500.0, 1.0)])
                .with_illumination_window(Some(1e1), Some(1e3)),
        ];
        set.regimes.push(Regime::new("Combined", vec![0, 1]));
        let config = set.regime_config(&set.regimes[0]);

        let gains = set.gating_gains_for(&config.curves, &[1e-2, 1e-2]);
        let product: f64 = gains.iter().product();
        // Regime gain = product of member gains: at 1e-2, the dim member
        // is fully on, the bright member fully off.
        assert!(product < 1e-3, "product {product}");
        // Bright member still inside its [1e1, 1e3] window at 50 W/m²
        // would be on too - probe it below its own 1e1 min instead.
        let dim_gains = set.gating_gains_for(&config.curves, &[1e-2, 5e-2]);
        assert!(
            dim_gains[0] > 0.9 && dim_gains[1] < 1e-3,
            "only the dim member should operate: {dim_gains:?}"
        );
    }

    #[test]
    fn gating_gains_use_per_curve_then_set_default_then_unbounded() {
        let mut set = sample_set();
        // No windows anywhere -> all gains 1 for any seen irradiance
        // (always on, as before).
        let all_on = sample_set().receptor_gains(&[1e-9, 1e9]);
        assert_eq!(all_on, vec![1.0, 1.0]);

        // Set-level default applies to curves without their own bounds.
        set.illumination_min = Some(1e-3);
        set.illumination_max = Some(1e-1);
        set.illumination_softness = Some(0.25);
        // Both receptors "see" 0.045 W/m² (mid-window) -> both on.
        let both_on = set.receptor_gains(&[0.045, 0.045]);
        assert!(
            both_on[0] > 0.9 && both_on[1] > 0.9,
            "both inside set window at 0.045 W/m²: {both_on:?}"
        );

        // A per-curve window overrides the set default.
        set.curves[0] = set.curves[0]
            .clone()
            .with_illumination_window(Some(1e1), Some(1e3));
        let overridden = set.receptor_gains(&[0.045, 0.045]);
        assert!(
            overridden[0] < 1e-3 && overridden[1] > 0.9,
            "curve 0 should use its own bright window: {overridden:?}"
        );
    }

    #[test]
    fn unaffiliated_receptor_gains_are_computed_independently() {
        let mut set = singleton_pool("with unaffiliated");
        set.curves.push(
            SpectralCurve::new("Unaffiliated", CurveType::Sensitivity)
                .with_illumination_window(Some(1e1), Some(1e3)),
        );
        set.illumination_min = Some(1e-3);
        set.illumination_max = Some(1e-1);
        // The unaffiliated curve has its own window, which wins over the
        // set default: at 100 W/m² seen it is mid-window...
        let mid = set.receptor_gains(&[0.0, 100.0]);
        assert!(mid[1] > 0.95, "own window wins: {mid:?}");
        // ...and far below its 1e1 min it is gated off.
        let dark = set.receptor_gains(&[0.0, 0.045]);
        assert!(dark[1] < 1e-3, "gated off below its own min: {dark:?}");
    }

    #[test]
    fn invalid_illumination_windows_are_flagged() {
        let mut set = sample_set();
        assert!(!set.has_invalid_illumination_window());

        set.curves[0].illumination_min = Some(10.0);
        set.curves[0].illumination_max = Some(1.0);
        assert!(set.has_invalid_illumination_window());
        set.curves[0] = SpectralCurve::new("S-cone", CurveType::Sensitivity);

        set.illumination_min = Some(-1.0);
        assert!(set.has_invalid_illumination_window());
    }

    #[test]
    fn illumination_window_fields_round_trip_with_the_set() {
        let mut set = sample_set();
        set.illumination_min = Some(0.01);
        set.illumination_max = Some(100.0);
        set.illumination_softness = Some(0.3);
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
        assert_eq!(loaded.regimes.len(), 1, "regimes survive a disk round trip");

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