//! Pure-Rust (multiband) GeoTIFF reading via the `tiff` crate (design
//! doc §3.4.1 point 2) - no native dependency, replacing GDAL for this
//! format (§10.3). No wavelength metadata is extracted here (baseline
//! TIFF has no such convention), same as what it replaces - §3.4.1
//! point 2's sensor-preset-dropdown fallback is what supplies it.

use std::fs::File;
use std::path::Path;

use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

#[derive(Debug, thiserror::Error)]
pub enum GeoTiffError {
    #[error("failed to open {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to decode {path}: {source}")]
    Decode {
        path: String,
        #[source]
        source: tiff::TiffError,
    },
    #[error(
        "{path} stores its bands in separate planes (PlanarConfiguration=2); only the \
         pixel-interleaved (chunky) layout is supported"
    )]
    PlanarNotSupported { path: String },
}

/// A decoded GeoTIFF, band-major (`data[band*lines*samples + line*samples
/// + sample]`) regardless of the file's own (chunky, pixel-interleaved)
/// on-disk order - matching `HyperspectralCube`'s layout.
pub struct GeoTiffCube {
    pub samples: usize,
    pub lines: usize,
    pub bands: usize,
    pub data: Vec<f32>,
}

/// Converts whatever numeric sample type the file actually stores
/// (TIFF allows several) into `f32`, matching how `.mat` reading
/// (`hyperspectral.rs`) already widens its own several numeric types.
fn to_f32_vec(result: DecodingResult) -> Vec<f32> {
    match result {
        DecodingResult::U8(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::U16(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::U32(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::U64(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::F32(v) => v,
        DecodingResult::F64(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I8(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I16(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I32(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I64(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::F16(v) => v.into_iter().map(|x| x.to_f32()).collect(),
    }
}

pub fn load_geotiff(path: impl AsRef<Path>) -> Result<GeoTiffCube, GeoTiffError> {
    let path = path.as_ref();
    let path_str = path.display().to_string();
    let file = File::open(path).map_err(|source| GeoTiffError::Io {
        path: path_str.clone(),
        source,
    })?;
    let mut decoder = Decoder::new(file).map_err(|source| GeoTiffError::Decode {
        path: path_str.clone(),
        source,
    })?;

    // PlanarConfiguration defaults to 1 (chunky) when the tag is absent,
    // per the TIFF 6.0 spec - only reject the explicit value 2 (planar).
    let planar_config = decoder
        .get_tag_u32(Tag::PlanarConfiguration)
        .unwrap_or(1);
    if planar_config == 2 {
        return Err(GeoTiffError::PlanarNotSupported { path: path_str });
    }

    let (width, height) = decoder.dimensions().map_err(|source| GeoTiffError::Decode {
        path: path_str.clone(),
        source,
    })?;
    // SamplesPerPixel defaults to 1 when absent, per the TIFF 6.0 spec.
    let bands = decoder
        .get_tag_u32(Tag::SamplesPerPixel)
        .unwrap_or(1) as usize;
    let (samples, lines) = (width as usize, height as usize);

    let image = decoder.read_image().map_err(|source| GeoTiffError::Decode {
        path: path_str.clone(),
        source,
    })?;
    let flat = to_f32_vec(image);

    // Chunky storage interleaves a pixel's `bands` samples contiguously,
    // row-major - i.e. exactly ENVI's BIP layout - so de-interleave into
    // this crate's band-major convention the same way `envi.rs` does.
    let mut data = vec![0.0_f32; samples * lines * bands];
    for l in 0..lines {
        for s in 0..samples {
            for b in 0..bands {
                let src = (l * samples + s) * bands + b;
                data[b * lines * samples + l * samples + s] = flat.get(src).copied().unwrap_or(0.0);
            }
        }
    }

    Ok(GeoTiffCube {
        samples,
        lines,
        bands,
        data,
    })
}

/// Hand-rolled minimal multiband float32 TIFF, written byte-by-byte,
/// independent of the decoder under test - uncompressed, chunky, one
/// strip. Deliberately not using any TIFF-writing crate (incl. `tiff`'s
/// own encoder) so this is a true independent fixture. `pub(crate)` so
/// `hyperspectral.rs`'s own dispatch-integration test can reuse it
/// instead of duplicating this byte layout.
#[cfg(test)]
pub(crate) fn write_minimal_tiff(path: &Path, samples: usize, lines: usize, bands: usize) {
    use std::io::Write;

    let mut pixel_data = Vec::new();
    for l in 0..lines {
        for s in 0..samples {
            for b in 0..bands {
                let value = (b * 100 + l * 10 + s) as f32;
                pixel_data.extend_from_slice(&value.to_le_bytes());
            }
        }
    }

    let mut f = std::fs::File::create(path).unwrap();
    let mut buf = Vec::new();
    buf.extend_from_slice(b"II"); // little-endian
    buf.extend_from_slice(&42u16.to_le_bytes()); // TIFF magic
    let ifd_offset_pos = buf.len();
    buf.extend_from_slice(&0u32.to_le_bytes()); // placeholder, patched below

    // Pixel data goes right after the header.
    let pixel_offset = buf.len() as u32;
    buf.extend_from_slice(&pixel_data);

    // IFD.
    let ifd_offset = buf.len() as u32;
    #[derive(Clone, Copy)]
    struct Entry {
        tag: u16,
        typ: u16,
        count: u32,
        value: u32,
    }
    let bits_per_sample = 32u32;
    let entries = [
        Entry { tag: 256, typ: 4, count: 1, value: samples as u32 }, // ImageWidth
        Entry { tag: 257, typ: 4, count: 1, value: lines as u32 },   // ImageLength
        Entry { tag: 258, typ: 3, count: 1, value: bits_per_sample }, // BitsPerSample
        Entry { tag: 259, typ: 3, count: 1, value: 1 },              // Compression: none
        Entry { tag: 262, typ: 3, count: 1, value: 1 },              // PhotometricInterpretation: BlackIsZero
        Entry { tag: 273, typ: 4, count: 1, value: pixel_offset },   // StripOffsets
        Entry { tag: 277, typ: 3, count: 1, value: bands as u32 },   // SamplesPerPixel
        Entry { tag: 278, typ: 4, count: 1, value: lines as u32 },   // RowsPerStrip (one strip)
        Entry {
            tag: 279,
            typ: 4,
            count: 1,
            value: pixel_data.len() as u32,
        }, // StripByteCounts
        Entry { tag: 339, typ: 3, count: 1, value: 3 }, // SampleFormat: IEEE float
    ];
    buf.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for e in &entries {
        buf.extend_from_slice(&e.tag.to_le_bytes());
        buf.extend_from_slice(&e.typ.to_le_bytes());
        buf.extend_from_slice(&e.count.to_le_bytes());
        // Short (type 3) values are stored left-justified in the 4-byte slot.
        if e.typ == 3 {
            buf.extend_from_slice(&(e.value as u16).to_le_bytes());
            buf.extend_from_slice(&0u16.to_le_bytes());
        } else {
            buf.extend_from_slice(&e.value.to_le_bytes());
        }
    }
    buf.extend_from_slice(&0u32.to_le_bytes()); // next IFD offset: none

    buf[ifd_offset_pos..ifd_offset_pos + 4].copy_from_slice(&ifd_offset.to_le_bytes());
    f.write_all(&buf).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "xenovision-geotiff-test-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn assert_matches_pattern(cube: &GeoTiffCube, samples: usize, lines: usize, bands: usize) {
        assert_eq!((cube.samples, cube.lines, cube.bands), (samples, lines, bands));
        for b in 0..bands {
            for l in 0..lines {
                for s in 0..samples {
                    let expected = (b * 100 + l * 10 + s) as f32;
                    let actual = cube.data[b * lines * samples + l * samples + s];
                    assert_eq!(actual, expected, "band {b} line {l} sample {s}");
                }
            }
        }
    }

    #[test]
    fn reads_a_single_band_tiff() {
        let dir = TempDir::new("1band");
        let path = dir.0.join("gray.tif");
        write_minimal_tiff(&path, 4, 3, 1);
        let cube = load_geotiff(&path).unwrap();
        assert_matches_pattern(&cube, 4, 3, 1);
    }

    #[test]
    fn reads_a_four_band_tiff() {
        let dir = TempDir::new("4band");
        let path = dir.0.join("multiband.tif");
        write_minimal_tiff(&path, 4, 3, 4);
        let cube = load_geotiff(&path).unwrap();
        assert_matches_pattern(&cube, 4, 3, 4);
    }

    #[test]
    fn reads_a_five_band_tiff() {
        let dir = TempDir::new("5band");
        let path = dir.0.join("multiband5.tif");
        write_minimal_tiff(&path, 3, 2, 5);
        let cube = load_geotiff(&path).unwrap();
        assert_matches_pattern(&cube, 3, 2, 5);
    }

    /// Writes a GeoTIFF with GDAL's own GTiff driver (a dev-dependency
    /// used only here) and confirms this pure-Rust reader is compatible
    /// with real GDAL output, not just this file's own hand-rolled
    /// fixtures above.
    #[test]
    fn reads_a_gdal_written_geotiff_identically() {
        let dir = TempDir::new("gdal");
        let path = dir.0.join("gdal.tif");
        let (samples, lines, bands) = (3usize, 2usize, 4usize);
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

        let cube = load_geotiff(&path).unwrap();
        assert_eq!((cube.samples, cube.lines, cube.bands), (samples, lines, bands));
        // band 4 (0-based 3), line 1, sample 2: 300 + line*samples+sample = 300+1*3+2
        let line = 1;
        assert_eq!(cube.data[3 * lines * samples + line * samples + 2], 305.0);
    }
}
