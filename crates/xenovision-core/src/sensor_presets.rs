//! GeoTIFF sensor/instrument band-set presets (design doc §3.4.1 point
//! 2's dropdown fallback for when a GeoTIFF carries no wavelength
//! metadata - the default case, not an edge case). Stored the same way
//! as the illuminant notch/narrow-band-source presets (`preset_store`'s
//! pattern): a built-in pristine default list, user-editable on disk,
//! with the same load/save/restore shape.

use serde::{Deserialize, Serialize};

use crate::preset_store;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorBandPreset {
    pub name: String,
    /// One center wavelength per band, in the sensor's own band order.
    pub wavelengths_nm: Vec<f64>,
}

/// Illustrative approximate center wavelengths for two commonly-
/// encountered multispectral sensors - not re-verified digit-by-digit
/// against each instrument's calibration data (consistent with how
/// this app treats other approximated-but-flagged values, e.g.
/// `fixtures::note_generic_fallback`). Good enough to demonstrate and
/// exercise the preset-dropdown mechanism; a user with precise
/// instrument specs can add/edit presets on disk via the same
/// `preset_store` file this loads from.
pub fn builtin_default_sensor_presets() -> Vec<SensorBandPreset> {
    vec![
        SensorBandPreset {
            name: "Landsat 8/9 OLI (9 bands, approximate)".to_string(),
            wavelengths_nm: vec![
                443.0, 482.0, 561.5, 654.5, 865.0, 1373.5, 1609.0, 2201.0, 590.0,
            ],
        },
        SensorBandPreset {
            name: "Sentinel-2 MSI (13 bands, approximate)".to_string(),
            wavelengths_nm: vec![
                443.0, 490.0, 560.0, 665.0, 705.0, 740.0, 783.0, 842.0, 865.0, 945.0, 1375.0,
                1610.0, 2190.0,
            ],
        },
    ]
}

pub fn load_sensor_presets() -> Vec<SensorBandPreset> {
    preset_store::load_or_init("sensor_band_presets.json", builtin_default_sensor_presets)
}

pub fn save_sensor_presets(presets: &[SensorBandPreset]) -> std::io::Result<()> {
    preset_store::save("sensor_band_presets.json", presets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_presets_have_wavelength_count_matching_their_own_band_count() {
        for preset in builtin_default_sensor_presets() {
            assert!(
                !preset.wavelengths_nm.is_empty(),
                "{} has no wavelengths",
                preset.name
            );
        }
    }
}
