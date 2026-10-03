//! Hyperspectral/multispectral image import and region extraction
//! ENVI and GeoTIFF cubes read via GDAL, MATLAB `.mat` hypercubes
//! read via the `matfile` crate - both validated in `spikes/spike_b`

use std::path::Path;

#[cfg(feature = "gdal")]
use gdal::{Dataset, Metadata};

use crate::curve::{CurveType, QuantityKind, SpectralCurve};

#[derive(Debug, thiserror::Error)]
pub enum HyperspectralError {
    #[cfg(feature = "gdal")]
    #[error("failed to open {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: gdal::errors::GdalError,
    },
    #[cfg(feature = "gdal")]
    #[error("failed to read band {band} of {path}: {source}")]
    Read {
        path: String,
        band: usize,
        #[source]
        source: gdal::errors::GdalError,
    },
    #[error(
        "can't open {path}: this build doesn't include ENVI/GeoTIFF support (GDAL). \
         MATLAB .mat files and text/CSV import still work."
    )]
    GdalUnavailable { path: String },
    #[error("failed to open {path}: {source}")]
    MatIo {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path} as a .mat file: {source}")]
    MatParse {
        path: String,
        #[source]
        source: matfile::Error,
    },
    #[error("variable \"{0}\" not found in the .mat file")]
    MatVariableNotFound(String),
    #[error("variable \"{0}\" has shape {1:?}, not a 3D array")]
    MatNotACube(String, Vec<usize>),
    #[error(
        "wavelength variable \"{name}\" has {found} elements, but the cube has {expected} bands"
    )]
    MatWavelengthLengthMismatch {
        name: String,
        expected: usize,
        found: usize,
    },
    #[error("{0} wavelengths given, but the cube has {1} bands")]
    WavelengthCountMismatch(usize, usize),
}

/// A loaded image cube: `bands` 2D images of `samples` × `lines` pixels
/// each (ENVI's own naming for width × height, kept here since both
/// ENVI and the `.mat` convention this module assumes are described in
/// those terms). `wavelengths_nm` is `None` until either read from
/// source metadata (ENVI's header, confirmed working in Spike B) or
/// explicitly assigned (GeoTIFF's sensor-preset dropdown fallback,
/// §3.4.1 point 2) - extraction requires it to be set.
pub struct HyperspectralCube {
    pub samples: usize,
    pub lines: usize,
    pub bands: usize,
    pub wavelengths_nm: Option<Vec<f64>>,
    /// Band-major: `data[band*lines*samples + line*samples + sample]`.
    data: Vec<f32>,
    /// Used to auto-label extracted curves (§3.4.2's "source image
    /// filename + region identifier").
    pub source_label: String,
}

impl HyperspectralCube {
    pub fn pixel(&self, line: usize, sample: usize, band: usize) -> f32 {
        self.data[band * self.lines * self.samples + line * self.samples + sample]
    }

    /// One band's `samples × lines` pixel grid, row-major - the slice an
    /// image-preview widget renders directly.
    pub fn band_slice(&self, band: usize) -> &[f32] {
        let start = band * self.lines * self.samples;
        &self.data[start..start + self.lines * self.samples]
    }

    /// Assigns a wavelength axis explicitly (the GeoTIFF sensor-preset/
    /// manual-entry fallback path, §3.4.1 point 2) - `Err` if the count
    /// doesn't match the cube's band count, rather than silently
    /// mismatching bands to the wrong wavelengths.
    pub fn assign_wavelengths(
        &mut self,
        wavelengths_nm: Vec<f64>,
    ) -> Result<(), HyperspectralError> {
        if wavelengths_nm.len() != self.bands {
            return Err(HyperspectralError::WavelengthCountMismatch(
                wavelengths_nm.len(),
                self.bands,
            ));
        }
        self.wavelengths_nm = Some(wavelengths_nm);
        Ok(())
    }
}

/// A region of a cube's `samples × lines` pixel grid, in pixel
/// coordinates (§3.4.2's "rectangle, polygon/lasso").
#[derive(Debug, Clone)]
pub enum Region {
    /// Corners in either order - normalized internally.
    Rectangle { x0: f64, y0: f64, x1: f64, y1: f64 },
    /// At least 3 vertices; implicitly closed (last vertex connects back
    /// to the first).
    Polygon(Vec<(f64, f64)>),
}

impl Region {
    fn contains(&self, x: f64, y: f64) -> bool {
        match self {
            Region::Rectangle { x0, y0, x1, y1 } => {
                let (xmin, xmax) = (x0.min(*x1), x0.max(*x1));
                let (ymin, ymax) = (y0.min(*y1), y0.max(*y1));
                x >= xmin && x < xmax && y >= ymin && y < ymax
            }
            Region::Polygon(points) => point_in_polygon(x, y, points),
        }
    }
}

/// Standard ray-casting point-in-polygon test.
fn point_in_polygon(x: f64, y: f64, points: &[(f64, f64)]) -> bool {
    if points.len() < 3 {
        return false;
    }
    let n = points.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = points[i];
        let (xj, yj) = points[j];
        if (yi > y) != (yj > y) {
            let x_intersect = (xj - xi) * (y - yi) / (yj - yi) + xi;
            if x < x_intersect {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

impl Region {
    /// Pixel-coordinate bounding box `(x_min, y_min, x_max, y_max)`.
    fn bounds(&self) -> (f64, f64, f64, f64) {
        match self {
            Region::Rectangle { x0, y0, x1, y1 } => {
                (x0.min(*x1), y0.min(*y1), x0.max(*x1), y0.max(*y1))
            }
            Region::Polygon(points) => points.iter().fold(
                (f64::MAX, f64::MAX, f64::MIN, f64::MIN),
                |(a, b, c, d), &(x, y)| (a.min(x), b.min(y), c.max(x), d.max(y)),
            ),
        }
    }
}

/// Worker count for the parallel loops below.
fn worker_count(items: usize) -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, items.max(1))
}

/// Indices (`line * samples + sample`, ascending) of every pixel whose
/// center falls inside `region`. Only the region's bounding box is
/// tested, split across threads by line.
fn region_pixel_indices(cube: &HyperspectralCube, region: &Region) -> Vec<usize> {
    let (x_min, y_min, x_max, y_max) = region.bounds();
    // A pixel's center is at +0.5, so these ranges cover every pixel that
    // could possibly be inside.
    let clamp = |v: f64, hi: usize| (v.max(0.0) as usize).min(hi);
    let line_range = clamp(y_min - 0.5, cube.lines)..clamp(y_max + 0.5, cube.lines);
    let sample_lo = clamp(x_min - 0.5, cube.samples);
    let sample_hi = clamp(x_max + 0.5, cube.samples);
    let lines: Vec<usize> = line_range.collect();
    if lines.is_empty() || sample_lo >= sample_hi {
        return Vec::new();
    }

    let chunk = lines.len().div_ceil(worker_count(lines.len()));
    std::thread::scope(|scope| {
        let handles: Vec<_> = lines
            .chunks(chunk)
            .map(|chunk_lines| {
                scope.spawn(move || {
                    let mut out = Vec::new();
                    for &line in chunk_lines {
                        for sample in sample_lo..sample_hi {
                            if region.contains(sample as f64 + 0.5, line as f64 + 0.5) {
                                out.push(line * cube.samples + sample);
                            }
                        }
                    }
                    out
                })
            })
            .collect();
        // Chunks are in line order, so concatenating keeps indices ascending.
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("region scan thread panicked"))
            .collect()
    })
}

/// Spatial-average extraction (§3.4.2): the mean value at each band
/// across every pixel whose center falls inside `region`. `None` if no
/// pixel matches (an empty or degenerate region).
///
/// The cube is band-major, so each band's pixels are one contiguous
/// slice; summing per band over a precomputed pixel list reads memory
/// sequentially, and bands are summed in parallel. Pixels are summed in
/// raster order within each band, as the original per-pixel loop did, so
/// the result is bit-for-bit the same.
pub fn extract_region_spectrum(cube: &HyperspectralCube, region: &Region) -> Option<Vec<f64>> {
    let pixels = region_pixel_indices(cube, region);
    if pixels.is_empty() {
        return None;
    }
    let count = pixels.len() as f64;
    let bands: Vec<usize> = (0..cube.bands).collect();
    let chunk = bands.len().div_ceil(worker_count(bands.len()));
    let pixels = &pixels;
    let means = std::thread::scope(|scope| {
        let handles: Vec<_> = bands
            .chunks(chunk)
            .map(|chunk_bands| {
                scope.spawn(move || {
                    chunk_bands
                        .iter()
                        .map(|&b| {
                            let slice = cube.band_slice(b);
                            let sum: f64 = pixels.iter().map(|&i| slice[i] as f64).sum();
                            sum / count
                        })
                        .collect::<Vec<f64>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("band sum thread panicked"))
            .collect()
    });
    Some(means)
}

/// Builds the extracted, auto-labeled Spectral Curve for one region
/// (§3.4.2's output + §5.2's "same mechanism, tagged Illumination"
/// variant as a labeling choice). `None` if the region matched no
/// pixels, or the cube has no wavelength axis assigned yet.
pub fn extracted_curve(
    cube: &HyperspectralCube,
    region: &Region,
    region_label: &str,
    as_illumination: bool,
) -> Option<SpectralCurve> {
    let spectrum = extract_region_spectrum(cube, region)?;
    let wavelengths = cube.wavelengths_nm.as_ref()?;
    let points: Vec<(f64, f64)> = wavelengths
        .iter()
        .zip(spectrum.iter())
        .map(|(&wl, &v)| (wl, v))
        .collect();
    let name = format!("{} — {region_label}", cube.source_label);
    // A light source is radiance. Image values are in sensor units, not
    // calibrated ones, hence "relative" - the same unit the built-in
    // luminants use, so deriving reflectance against them works directly.
    let (curve_type, quantity) = if as_illumination {
        (
            CurveType::Illumination,
            QuantityKind::Radiance {
                unit: "relative".to_string(),
            },
        )
    } else {
        (CurveType::Reflectance, QuantityKind::Reflectance)
    };
    let mut curve = SpectralCurve::new(name, curve_type)
        .with_points(points)
        .with_quantity(quantity);
    curve.metadata.insert(
        "source".to_string(),
        format!(
            "Extracted from \"{}\", region {region_label}",
            cube.source_label
        ),
    );
    Some(curve)
}

fn source_label_for(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string()
}

/// Opens an ENVI or GeoTIFF file via GDAL (format auto-detected by
/// GDAL itself) and reads every band into a `HyperspectralCube`,
/// pulling `wavelength = ...` header metadata into `wavelengths_nm`
/// when every band has it (ENVI; confirmed working in Spike B) and
/// leaving it `None` otherwise (GeoTIFF's typical case, per Spike B).
#[cfg(feature = "gdal")]
pub fn load_envi_or_geotiff(
    path: impl AsRef<Path>,
) -> Result<HyperspectralCube, HyperspectralError> {
    let path = path.as_ref();
    let path_str = path.display().to_string();
    let dataset = Dataset::open(path).map_err(|source| HyperspectralError::Open {
        path: path_str.clone(),
        source,
    })?;
    let (samples, lines) = dataset.raster_size();
    let bands = dataset.raster_count();

    let mut data = vec![0.0_f32; samples * lines * bands];
    let mut per_band_wavelength: Vec<Option<f64>> = Vec::with_capacity(bands);
    for b in 1..=bands {
        let band = dataset
            .rasterband(b)
            .map_err(|source| HyperspectralError::Read {
                path: path_str.clone(),
                band: b,
                source,
            })?;
        let buf = band
            .read_as::<f32>((0, 0), (samples, lines), (samples, lines), None)
            .map_err(|source| HyperspectralError::Read {
                path: path_str.clone(),
                band: b,
                source,
            })?;
        let dest_start = (b - 1) * lines * samples;
        data[dest_start..dest_start + lines * samples].copy_from_slice(buf.data());

        let wavelength = band
            .metadata_domain("")
            .unwrap_or_default()
            .iter()
            .find_map(|item| {
                item.strip_prefix("wavelength=")
                    .and_then(|v| v.trim().parse::<f64>().ok())
            });
        per_band_wavelength.push(wavelength);
    }
    let wavelengths_nm = if per_band_wavelength.iter().all(Option::is_some) {
        Some(
            per_band_wavelength
                .into_iter()
                .map(Option::unwrap)
                .collect(),
        )
    } else {
        None
    };

    Ok(HyperspectralCube {
        samples,
        lines,
        bands,
        wavelengths_nm,
        data,
        source_label: source_label_for(path),
    })
}

/// Without the `gdal` feature, ENVI/GeoTIFF can't be read; this explains
/// why instead of the app silently lacking the option.
#[cfg(not(feature = "gdal"))]
pub fn load_envi_or_geotiff(
    path: impl AsRef<Path>,
) -> Result<HyperspectralCube, HyperspectralError> {
    Err(HyperspectralError::GdalUnavailable {
        path: path.as_ref().display().to_string(),
    })
}

/// Whether this build can read ENVI/GeoTIFF files (the `gdal` feature).
pub const GDAL_AVAILABLE: bool = cfg!(feature = "gdal");

/// One variable found in a `.mat` file: its name and shape, as reported
/// by `matfile` (which - per Spike B - only ever lists variables of
/// supported, decodable types; non-numeric variables are silently
/// absent rather than erroring).
pub struct MatVariableInfo {
    pub name: String,
    pub shape: Vec<usize>,
}

pub fn list_mat_variables(
    path: impl AsRef<Path>,
) -> Result<Vec<MatVariableInfo>, HyperspectralError> {
    let path = path.as_ref();
    let path_str = path.display().to_string();
    let file = std::fs::File::open(path).map_err(|source| HyperspectralError::MatIo {
        path: path_str.clone(),
        source,
    })?;
    let mat = matfile::MatFile::parse(file).map_err(|source| HyperspectralError::MatParse {
        path: path_str,
        source,
    })?;
    Ok(mat
        .arrays()
        .iter()
        .map(|a| MatVariableInfo {
            name: a.name().to_string(),
            shape: a.size().clone(),
        })
        .collect())
}

/// The §3.4.1 point 3 shape-heuristic auto-guess, pre-filling the
/// confirmation dropdown: a 3D array is proposed as the cube, and -
/// corrected per Spike B's finding that MATLAB has no true 1D arrays -
/// a 2D array with one dimension `== 1` and the other matching the
/// cube's band count is proposed as the wavelength vector.
pub struct MatGuess {
    pub cube_name: Option<String>,
    pub wavelength_name: Option<String>,
}

pub fn guess_mat_mapping(variables: &[MatVariableInfo]) -> MatGuess {
    let cube = variables.iter().find(|v| v.shape.len() == 3);
    let cube_name = cube.map(|v| v.name.clone());
    let wavelength_name = cube.and_then(|c| {
        let band_dim = *c.shape.last().unwrap();
        variables
            .iter()
            .find(|v| {
                v.shape.len() == 2
                    && ((v.shape[0] == 1 && v.shape[1] == band_dim)
                        || (v.shape[1] == 1 && v.shape[0] == band_dim))
            })
            .map(|v| v.name.clone())
    });
    MatGuess {
        cube_name,
        wavelength_name,
    }
}

fn numeric_data_to_f64(data: &matfile::NumericData) -> Vec<f64> {
    use matfile::NumericData as N;
    match data {
        N::Int8 { real, .. } => real.iter().map(|&v| v as f64).collect(),
        N::UInt8 { real, .. } => real.iter().map(|&v| v as f64).collect(),
        N::Int16 { real, .. } => real.iter().map(|&v| v as f64).collect(),
        N::UInt16 { real, .. } => real.iter().map(|&v| v as f64).collect(),
        N::Int32 { real, .. } => real.iter().map(|&v| v as f64).collect(),
        N::UInt32 { real, .. } => real.iter().map(|&v| v as f64).collect(),
        N::Int64 { real, .. } => real.iter().map(|&v| v as f64).collect(),
        N::UInt64 { real, .. } => real.iter().map(|&v| v as f64).collect(),
        N::Single { real, .. } => real.iter().map(|&v| v as f64).collect(),
        N::Double { real, .. } => real.clone(),
    }
}

/// Loads a `HyperspectralCube` from a confirmed `.mat` cube variable
/// (and, optionally, a confirmed wavelength variable) - the final step
/// after `list_mat_variables`/`guess_mat_mapping`'s confirmation
/// dropdown. **Dimension-order assumption** (not detected, not
/// specified by the design doc): the cube's 3 dimensions are taken as
/// `[lines, samples, bands]`, matching this module's ENVI convention -
/// flagged here rather than silently assumed without documentation.
pub fn load_mat_cube(
    path: impl AsRef<Path>,
    cube_name: &str,
    wavelength_name: Option<&str>,
) -> Result<HyperspectralCube, HyperspectralError> {
    let path = path.as_ref();
    let path_str = path.display().to_string();
    let file = std::fs::File::open(path).map_err(|source| HyperspectralError::MatIo {
        path: path_str.clone(),
        source,
    })?;
    let mat = matfile::MatFile::parse(file).map_err(|source| HyperspectralError::MatParse {
        path: path_str,
        source,
    })?;

    let cube_array = mat
        .find_by_name(cube_name)
        .ok_or_else(|| HyperspectralError::MatVariableNotFound(cube_name.to_string()))?;
    let dims = cube_array.size().clone();
    if dims.len() != 3 {
        return Err(HyperspectralError::MatNotACube(cube_name.to_string(), dims));
    }
    let (lines, samples, bands) = (dims[0], dims[1], dims[2]);
    let flat = numeric_data_to_f64(cube_array.data());

    // MATLAB stores arrays column-major (first dimension varies
    // fastest); reorder into this module's band-major convention.
    let mut data = vec![0.0_f32; lines * samples * bands];
    for b in 0..bands {
        for s in 0..samples {
            for l in 0..lines {
                let src_idx = l + s * lines + b * lines * samples;
                let dst_idx = b * lines * samples + l * samples + s;
                data[dst_idx] = flat[src_idx] as f32;
            }
        }
    }

    let wavelengths_nm = match wavelength_name {
        Some(name) => {
            let wl_array = mat
                .find_by_name(name)
                .ok_or_else(|| HyperspectralError::MatVariableNotFound(name.to_string()))?;
            let wl_flat = numeric_data_to_f64(wl_array.data());
            if wl_flat.len() != bands {
                return Err(HyperspectralError::MatWavelengthLengthMismatch {
                    name: name.to_string(),
                    expected: bands,
                    found: wl_flat.len(),
                });
            }
            Some(wl_flat)
        }
        None => None,
    };

    Ok(HyperspectralCube {
        samples,
        lines,
        bands,
        wavelengths_nm,
        data,
        source_label: source_label_for(path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_cube() -> HyperspectralCube {
        // 2 samples x 2 lines x 3 bands: pixel value = band*100 + line*10 + sample.
        let (samples, lines, bands) = (2, 2, 3);
        let mut data = vec![0.0_f32; samples * lines * bands];
        for b in 0..bands {
            for l in 0..lines {
                for s in 0..samples {
                    data[b * lines * samples + l * samples + s] = (b * 100 + l * 10 + s) as f32;
                }
            }
        }
        HyperspectralCube {
            samples,
            lines,
            bands,
            wavelengths_nm: Some(vec![400.0, 500.0, 600.0]),
            data,
            source_label: "test".to_string(),
        }
    }

    #[test]
    fn pixel_and_band_slice_match_hand_layout() {
        let cube = tiny_cube();
        assert_eq!(cube.pixel(0, 0, 0), 0.0);
        assert_eq!(cube.pixel(1, 1, 2), 211.0); // band 2, line 1, sample 1
        assert_eq!(cube.band_slice(1), &[100.0, 101.0, 110.0, 111.0]);
    }

    #[test]
    fn assign_wavelengths_rejects_wrong_length() {
        let mut cube = tiny_cube();
        assert!(matches!(
            cube.assign_wavelengths(vec![1.0, 2.0]),
            Err(HyperspectralError::WavelengthCountMismatch(2, 3))
        ));
        assert!(cube.assign_wavelengths(vec![1.0, 2.0, 3.0]).is_ok());
        assert_eq!(cube.wavelengths_nm, Some(vec![1.0, 2.0, 3.0]));
    }

    #[test]
    fn rectangle_region_extracts_mean_over_contained_pixels() {
        let cube = tiny_cube();
        // Rectangle covering just the single pixel (sample=1, line=0).
        let region = Region::Rectangle {
            x0: 1.0,
            y0: 0.0,
            x1: 2.0,
            y1: 1.0,
        };
        let spectrum = extract_region_spectrum(&cube, &region).unwrap();
        assert_eq!(spectrum, vec![1.0, 101.0, 201.0]);
    }

    #[test]
    fn rectangle_region_covering_whole_cube_averages_all_four_pixels() {
        let cube = tiny_cube();
        let region = Region::Rectangle {
            x0: 0.0,
            y0: 0.0,
            x1: 2.0,
            y1: 2.0,
        };
        let spectrum = extract_region_spectrum(&cube, &region).unwrap();
        // Band 0 pixels: 0,1,10,11 -> mean 5.5.
        assert!((spectrum[0] - 5.5).abs() < 1e-9);
        assert!((spectrum[1] - 105.5).abs() < 1e-9);
    }

    #[test]
    fn empty_region_returns_none_rather_than_a_fabricated_zero() {
        let cube = tiny_cube();
        let region = Region::Rectangle {
            x0: 50.0,
            y0: 50.0,
            x1: 60.0,
            y1: 60.0,
        };
        assert_eq!(extract_region_spectrum(&cube, &region), None);
    }

    /// The original per-pixel extraction loop, kept as a reference.
    fn extract_reference(cube: &HyperspectralCube, region: &Region) -> Option<Vec<f64>> {
        let mut sums = vec![0.0_f64; cube.bands];
        let mut count = 0_usize;
        for line in 0..cube.lines {
            for sample in 0..cube.samples {
                if region.contains(sample as f64 + 0.5, line as f64 + 0.5) {
                    count += 1;
                    for (b, sum) in sums.iter_mut().enumerate() {
                        *sum += cube.pixel(line, sample, b) as f64;
                    }
                }
            }
        }
        (count > 0).then(|| sums.into_iter().map(|s| s / count as f64).collect())
    }

    #[test]
    fn fast_extraction_matches_reference_exactly() {
        let (samples, lines, bands) = (37, 23, 9);
        let data: Vec<f32> = (0..samples * lines * bands)
            .map(|i| ((i * 7919) % 1013) as f32 * 0.37 - 50.0)
            .collect();
        let cube = HyperspectralCube {
            samples,
            lines,
            bands,
            wavelengths_nm: None,
            data,
            source_label: "t".into(),
        };
        let regions = [
            Region::Rectangle {
                x0: 0.0,
                y0: 0.0,
                x1: 37.0,
                y1: 23.0,
            },
            Region::Rectangle {
                x0: 30.2,
                y0: 19.7,
                x1: 3.4,
                y1: 2.1,
            },
            Region::Rectangle {
                x0: -10.0,
                y0: -5.0,
                x1: 50.0,
                y1: 40.0,
            },
            Region::Rectangle {
                x0: 4.0,
                y0: 4.0,
                x1: 5.0,
                y1: 5.0,
            },
            Region::Rectangle {
                x0: 4.2,
                y0: 4.2,
                x1: 4.3,
                y1: 4.3,
            },
            Region::Polygon(vec![(1.0, 1.0), (35.0, 4.0), (20.0, 22.0)]),
            Region::Polygon(vec![(-5.0, 10.0), (18.0, -3.0), (45.0, 12.0), (18.0, 30.0)]),
            Region::Polygon(
                (0..40)
                    .map(|k| {
                        let a = k as f64 / 40.0 * std::f64::consts::TAU;
                        (18.5 + 15.0 * a.cos(), 11.5 + 9.0 * (3.0 * a).sin())
                    })
                    .collect(),
            ),
        ];
        for region in &regions {
            assert_eq!(
                extract_region_spectrum(&cube, region),
                extract_reference(&cube, region),
                "{region:?}"
            );
        }
    }

    #[test]
    fn polygon_region_matches_rectangle_for_a_rectangular_polygon() {
        let cube = tiny_cube();
        let rect = Region::Rectangle {
            x0: 0.0,
            y0: 0.0,
            x1: 2.0,
            y1: 2.0,
        };
        let polygon = Region::Polygon(vec![(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)]);
        assert_eq!(
            extract_region_spectrum(&cube, &rect),
            extract_region_spectrum(&cube, &polygon)
        );
    }

    #[test]
    fn extracted_curve_is_none_without_a_wavelength_axis() {
        let mut cube = tiny_cube();
        cube.wavelengths_nm = None;
        let region = Region::Rectangle {
            x0: 0.0,
            y0: 0.0,
            x1: 2.0,
            y1: 2.0,
        };
        assert!(extracted_curve(&cube, &region, "test region", false).is_none());
    }

    #[test]
    fn extracted_curve_tags_illumination_variant_per_toggle() {
        let cube = tiny_cube();
        let region = Region::Rectangle {
            x0: 0.0,
            y0: 0.0,
            x1: 2.0,
            y1: 2.0,
        };
        let reflectance = extracted_curve(&cube, &region, "r1", false).unwrap();
        assert_eq!(reflectance.curve_type, CurveType::Reflectance);
        assert_eq!(reflectance.quantity, QuantityKind::Reflectance);
        let illumination = extracted_curve(&cube, &region, "r1", true).unwrap();
        assert_eq!(illumination.curve_type, CurveType::Illumination);
        assert_eq!(
            illumination.quantity,
            QuantityKind::Radiance {
                unit: "relative".to_string()
            }
        );
        assert_eq!(reflectance.points.len(), 3);
    }

    #[test]
    fn guess_mat_mapping_prefers_vector_shaped_wavelength_match() {
        let variables = vec![
            MatVariableInfo {
                name: "hypercube".to_string(),
                shape: vec![3, 4, 5],
            },
            MatVariableInfo {
                name: "wavelengths".to_string(),
                shape: vec![1, 5],
            },
            MatVariableInfo {
                name: "unrelated".to_string(),
                shape: vec![2, 2],
            },
        ];
        let guess = guess_mat_mapping(&variables);
        assert_eq!(guess.cube_name, Some("hypercube".to_string()));
        assert_eq!(guess.wavelength_name, Some("wavelengths".to_string()));
    }

    #[test]
    fn guess_mat_mapping_is_none_when_no_3d_array_present() {
        let variables = vec![MatVariableInfo {
            name: "flat".to_string(),
            shape: vec![2, 2],
        }];
        let guess = guess_mat_mapping(&variables);
        assert_eq!(guess.cube_name, None);
        assert_eq!(guess.wavelength_name, None);
    }

    /// End-to-end against a GDAL-written ENVI file, mirroring
    /// Spike B's own probe but as a proper regression test: confirms
    /// `load_envi_or_geotiff` extracts dimensions, per-band wavelength
    /// metadata, and pixel data correctly through this module's actual
    /// (non-spike) code path.
    #[cfg(not(feature = "gdal"))]
    #[test]
    fn without_gdal_envi_geotiff_load_reports_why() {
        const { assert!(!GDAL_AVAILABLE) };
        let Err(err) = load_envi_or_geotiff("/tmp/some_image.hdr") else {
            panic!("loading must fail without GDAL");
        };
        assert!(matches!(err, HyperspectralError::GdalUnavailable { .. }));
        assert!(err.to_string().contains("GDAL"));
    }

    #[cfg(feature = "gdal")]
    #[test]
    fn load_envi_or_geotiff_reads_a_real_envi_file_correctly() {
        let dir = std::env::temp_dir().join(format!(
            "xenovision-hyperspectral-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dat_path = dir.join("sample.dat");
        let hdr_path = dir.join("sample.hdr");

        let (samples, lines, bands) = (3, 2, 4);
        let mut bytes = Vec::new();
        for b in 0..bands {
            for l in 0..lines {
                for s in 0..samples {
                    let value = (b * 100 + l * 10 + s) as f32;
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        std::fs::write(&dat_path, &bytes).unwrap();
        let wavelengths: Vec<String> = (0..bands)
            .map(|b| format!("{}", 400.0 + b as f64 * 50.0))
            .collect();
        let hdr = format!(
            "ENVI\nsamples = {samples}\nlines = {lines}\nbands = {bands}\nheader offset = 0\nfile type = ENVI Standard\ndata type = 4\ninterleave = bsq\nbyte order = 0\nwavelength = {{{}}}\n",
            wavelengths.join(", ")
        );
        std::fs::write(&hdr_path, hdr).unwrap();

        let cube = load_envi_or_geotiff(&dat_path).unwrap();
        assert_eq!((cube.samples, cube.lines, cube.bands), (3, 2, 4));
        assert_eq!(cube.wavelengths_nm, Some(vec![400.0, 450.0, 500.0, 550.0]));
        assert_eq!(cube.pixel(1, 2, 3), 312.0); // band 3, line 1, sample 2

        std::fs::remove_file(&dat_path).ok();
        std::fs::remove_file(&hdr_path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    /// Mirrors the ENVI test above, but for GeoTIFF: writes a
    /// multiband GeoTIFF via GDAL's own GTiff driver, then confirms
    /// `load_envi_or_geotiff` reads the pixel data back correctly and -
    /// since this file carries no wavelength tags - leaves
    /// `wavelengths_nm` at `None` rather than fabricating one.
    #[cfg(feature = "gdal")]
    #[test]
    fn load_envi_or_geotiff_reads_a_real_geotiff_file_correctly() {
        let dir = std::env::temp_dir().join(format!(
            "xenovision-hyperspectral-geotiff-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.tif");

        let (samples, lines, bands) = (3_usize, 2_usize, 4_usize);
        {
            let driver = gdal::DriverManager::get_driver_by_name("GTiff").unwrap();
            let dataset = driver
                .create_with_band_type::<f32, _>(&path, samples, lines, bands)
                .unwrap();
            for b in 1..=bands {
                let mut band = dataset.rasterband(b).unwrap();
                let pixel_data: Vec<f32> = (0..lines * samples)
                    .map(|i| ((b - 1) * 100 + i) as f32)
                    .collect();
                let mut buffer = gdal::raster::Buffer::new((samples, lines), pixel_data);
                band.write((0, 0), (samples, lines), &mut buffer).unwrap();
            }
        }

        let cube = load_envi_or_geotiff(&path).unwrap();
        assert_eq!((cube.samples, cube.lines, cube.bands), (3, 2, 4));
        assert_eq!(
            cube.wavelengths_nm, None,
            "GeoTIFF has no wavelength tags here"
        );
        assert_eq!(cube.pixel(1, 2, 3), 305.0); // band 4 (0-based 3): 300 + line*samples+sample = 300+1*3+2

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }
}
