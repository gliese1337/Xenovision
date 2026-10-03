//! Spike B (design doc §3.4, impl-plan Phase 0): validate that
//! `georust/gdal` can read ENVI (all 3 interleaves) and GeoTIFF, with
//! wavelength metadata extraction, and that a candidate `.mat` crate can
//! list variables/shapes in a hypercube file - before building any real
//! import UX on top of either.

use std::io::Write;
use std::path::Path;

use gdal::{Dataset, Metadata};

fn write_envi_sample(
    dir: &Path,
    interleave: &str,
    samples: usize,
    lines: usize,
    bands: usize,
) -> std::path::PathBuf {
    let stem = dir.join(format!("sample_{interleave}"));
    let dat_path = stem.with_extension("dat");
    let hdr_path = stem.with_extension("hdr");

    // Deterministic float32 data: value = band*100 + row*10 + col, laid
    // out according to the requested interleave scheme.
    let value_at =
        |row: usize, col: usize, band: usize| -> f32 { (band * 100 + row * 10 + col) as f32 };

    let mut bytes = Vec::with_capacity(samples * lines * bands * 4);
    match interleave {
        "bsq" => {
            for b in 0..bands {
                for r in 0..lines {
                    for c in 0..samples {
                        bytes.extend_from_slice(&value_at(r, c, b).to_le_bytes());
                    }
                }
            }
        }
        "bil" => {
            for r in 0..lines {
                for b in 0..bands {
                    for c in 0..samples {
                        bytes.extend_from_slice(&value_at(r, c, b).to_le_bytes());
                    }
                }
            }
        }
        "bip" => {
            for r in 0..lines {
                for c in 0..samples {
                    for b in 0..bands {
                        bytes.extend_from_slice(&value_at(r, c, b).to_le_bytes());
                    }
                }
            }
        }
        _ => panic!("unknown interleave {interleave}"),
    }
    std::fs::write(&dat_path, &bytes).unwrap();

    let wavelengths: Vec<String> = (0..bands)
        .map(|b| format!("{}", 400.0 + b as f64 * 50.0))
        .collect();
    let hdr = format!(
        "ENVI\nsamples = {samples}\nlines = {lines}\nbands = {bands}\nheader offset = 0\nfile type = ENVI Standard\ndata type = 4\ninterleave = {interleave}\nbyte order = 0\nwavelength = {{{}}}\n",
        wavelengths.join(", ")
    );
    std::fs::write(&hdr_path, hdr).unwrap();
    dat_path
}

fn probe_envi(dir: &Path, interleave: &str) {
    println!("\n--- ENVI, interleave={interleave} ---");
    let path = write_envi_sample(dir, interleave, 4, 3, 5);
    let ds = match Dataset::open(&path) {
        Ok(ds) => ds,
        Err(e) => {
            println!("FAILED to open: {e}");
            return;
        }
    };
    println!(
        "size={:?} raster_count={}",
        ds.raster_size(),
        ds.raster_count()
    );
    for b in 1..=ds.raster_count() {
        let band = ds.rasterband(b).unwrap();
        let wavelength_keys: Vec<String> = band
            .metadata_domains()
            .into_iter()
            .flat_map(|domain| {
                band.metadata_domain(&domain)
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |item| format!("[{domain}] {item}"))
            })
            .collect();
        println!("band {b} metadata items: {wavelength_keys:?}");
    }
    println!(
        "dataset-level metadata (domain \"\"): {:?}",
        ds.metadata_domain("").unwrap_or_default()
    );

    // Confirm pixel values round-trip correctly for this interleave by
    // reading band 3 (index 2, value = 2*100 + row*10 + col) back out.
    let band = ds.rasterband(3).unwrap();
    let buf = band
        .read_as::<f32>((0, 0), ds.raster_size(), ds.raster_size(), None)
        .unwrap();
    let expected_top_left = 200.0_f32;
    let expected_bottom_right = 200.0 + 2.0 * 10.0 + 3.0;
    println!(
        "band 3 top-left={} (expect {expected_top_left}), bottom-right={} (expect {expected_bottom_right})",
        buf.data()[0],
        buf.data()[buf.data().len() - 1]
    );
}

fn probe_geotiff(dir: &Path) {
    println!("\n--- GeoTIFF, no wavelength metadata ---");
    let path = dir.join("sample.tif");
    {
        let driver = gdal::DriverManager::get_driver_by_name("GTiff").unwrap();
        let ds = driver
            .create_with_band_type::<f32, _>(&path, 4, 3, 3)
            .unwrap();
        for b in 1..=3 {
            let mut band = ds.rasterband(b).unwrap();
            let data: Vec<f32> = (0..12).map(|i| (b * 100 + i) as f32).collect();
            let mut buffer = gdal::raster::Buffer::new((4, 3), data);
            band.write((0, 0), (4, 3), &mut buffer).unwrap();
        }
    }
    let ds = Dataset::open(&path).unwrap();
    println!(
        "size={:?} raster_count={}",
        ds.raster_size(),
        ds.raster_count()
    );
    println!(
        "dataset-level metadata domains: {:?}",
        ds.metadata_domains()
    );
    println!(
        "any \"wavelength\" tag present anywhere? (expected: no - this is the \
         case the design doc's sensor-preset dropdown fallback exists for)"
    );
}

fn probe_mat_file(dir: &Path) {
    println!("\n--- .mat hypercube (matfile crate) ---");
    // matfile has no write support (confirmed by reading its source -
    // "Writing .mat files" is an unchecked box in its own docs), so this
    // spike's sample file is generated by scipy.io.savemat (a
    // MATLAB-compatible v5 writer) rather than round-tripped through
    // matfile itself.
    let path = dir.join("sample_cube.mat");
    if !path.exists() {
        println!(
            "SKIPPED: {} not found (expected to be generated by a one-off \
             `python3 -c \"...scipy.io.savemat...\"` step before running this spike)",
            path.display()
        );
        return;
    }
    let file = std::fs::File::open(&path).unwrap();
    let mat_file = match matfile::MatFile::parse(file) {
        Ok(m) => m,
        Err(e) => {
            println!("FAILED to parse: {e}");
            return;
        }
    };
    println!("Variables found:");
    for array in mat_file.arrays() {
        println!("  name={:?} size={:?}", array.name(), array.size());
    }

    // The shape-heuristic the design doc specifies, literally: a 3D
    // array is the cube candidate, a 1D array whose length matches the
    // cube's band dimension is the wavelength-vector candidate.
    let cube = mat_file.arrays().iter().find(|a| a.size().len() == 3);
    let Some(cube) = cube else {
        println!("heuristic FAILED: no 3D array found");
        return;
    };
    let band_dim = *cube.size().last().unwrap();
    let literal_1d_match = mat_file
        .arrays()
        .iter()
        .find(|a| a.size().len() == 1 && a.size()[0] == band_dim);
    println!(
        "literal \"ndims==1\" heuristic: cube={:?} (shape {:?}), wavelengths={:?}",
        cube.name(),
        cube.size(),
        literal_1d_match.map(|a| (a.name(), a.size()))
    );

    // MATLAB has no true 1D arrays - a vector saved from a 1D source
    // array round-trips as a 1xN or Nx1 2D array (confirmed above:
    // "wavelengths" came back as size [1, 5], not [5]). The heuristic
    // actually needs to accept "exactly one dimension equals 1, and the
    // other equals the cube's band count" instead of literal ndims==1.
    let vector_like_match = mat_file.arrays().iter().find(|a| {
        a.size().len() == 2
            && ((a.size()[0] == 1 && a.size()[1] == band_dim)
                || (a.size()[1] == 1 && a.size()[0] == band_dim))
    });
    println!(
        "corrected \"vector-like\" heuristic: wavelengths={:?}",
        vector_like_match.map(|a| (a.name(), a.size()))
    );
}

fn main() {
    let dir = std::env::temp_dir().join("xenovision-spike-b");
    std::fs::create_dir_all(&dir).unwrap();
    println!("Working dir: {}", dir.display());

    for interleave in ["bsq", "bil", "bip"] {
        probe_envi(&dir, interleave);
    }
    probe_geotiff(&dir);
    probe_mat_file(&dir);

    let _ = std::io::stdout().flush();
}
