//! Pure-Rust ENVI header (`.hdr`) parsing and raw BSQ/BIL/BIP
//! de-interleaving (design doc §3.4.1 point 1) - no native dependency,
//! replacing GDAL for this format (§10.3).
//!
//! An ENVI header is a plain-text key/value file. Most values are a
//! single token on one line (`samples = 100`); list values are wrapped
//! in `{ }`, which may itself span several lines (a wavelength list is
//! the common case long enough to be wrapped that way). Keys are
//! matched case-insensitively, per the format's own convention.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum EnviError {
    #[error("failed to read header {path}: {source}")]
    HeaderIo {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read data file {path}: {source}")]
    DataIo {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("no ENVI header found alongside {0} (tried a sibling \"{1}\")")]
    HeaderNotFound(String, String),
    #[error("no ENVI data file found alongside header {0} (tried: {1})")]
    DataFileNotFound(String, String),
    #[error("header {path} is missing required field \"{field}\"")]
    MissingField { path: String, field: String },
    #[error("header {path} field \"{field}\" = \"{value}\" isn't a valid {expected}")]
    InvalidField {
        path: String,
        field: String,
        value: String,
        expected: String,
    },
    #[error(
        "header {path} declares data type {code}, which this reader doesn't support \
         (supported: 1=byte, 2=int16, 3=int32, 4=float32, 5=float64, 12=uint16, 13=uint32)"
    )]
    UnsupportedDataType { path: String, code: i64 },
    #[error("data file {path} is {found} bytes, too short for the {expected}-byte cube the header describes")]
    DataFileTooShort {
        path: String,
        expected: usize,
        found: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interleave {
    Bsq,
    Bil,
    Bip,
}

/// A parsed ENVI header, plus the pixel data it describes - already
/// de-interleaved into band-major (BSQ) order, matching
/// `HyperspectralCube`'s own internal layout, so the caller never has
/// to think about the file's original interleave again.
#[derive(Debug)]
pub struct EnviCube {
    pub samples: usize,
    pub lines: usize,
    pub bands: usize,
    pub wavelengths_nm: Option<Vec<f64>>,
    /// Band-major: `data[band*lines*samples + line*samples + sample]`.
    pub data: Vec<f32>,
}

/// Parses `header_text` into lowercase-keyed entries, joining any
/// `{ ... }` list value that spans multiple lines into one string
/// (braces stripped) before returning it.
fn parse_header(header_text: &str) -> HashMap<String, String> {
    let mut entries = HashMap::new();
    let mut lines = header_text.lines();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line.is_empty() || line.eq_ignore_ascii_case("ENVI") || line.starts_with(';') {
            continue;
        }
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_lowercase();
        let mut value = rest.trim().to_string();
        if value.starts_with('{') && !value.contains('}') {
            for cont in lines.by_ref() {
                value.push(' ');
                value.push_str(cont.trim());
                if cont.contains('}') {
                    break;
                }
            }
        }
        let value = value.trim();
        let value = value.strip_prefix('{').unwrap_or(value);
        let value = value.strip_suffix('}').unwrap_or(value);
        entries.insert(key, value.trim().to_string());
    }
    entries
}

fn required_usize(
    entries: &HashMap<String, String>,
    field: &str,
    path: &str,
) -> Result<usize, EnviError> {
    let raw = entries
        .get(field)
        .ok_or_else(|| EnviError::MissingField {
            path: path.to_string(),
            field: field.to_string(),
        })?;
    raw.trim()
        .parse::<usize>()
        .map_err(|_| EnviError::InvalidField {
            path: path.to_string(),
            field: field.to_string(),
            value: raw.clone(),
            expected: "non-negative integer".to_string(),
        })
}

/// Candidate sibling data-file extensions tried (in order) when the
/// caller points at a `.hdr` file directly - `.hdr` itself is never a
/// data file, so one of these (or no extension at all) must exist.
const DATA_EXTENSIONS: &[&str] = &["dat", "img", "raw", "bin"];

/// Resolves `input` (either the `.hdr` header or its paired data file,
/// matching how both this and the GDAL driver it replaces accept
/// either) into `(header_path, data_path)`.
fn resolve_paths(input: &Path) -> Result<(PathBuf, PathBuf), EnviError> {
    let is_hdr = input
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("hdr"));

    if is_hdr {
        let stem_base = input.with_extension("");
        for ext in DATA_EXTENSIONS {
            let candidate = stem_base.with_extension(ext);
            if candidate.is_file() {
                return Ok((input.to_path_buf(), candidate));
            }
        }
        if stem_base.is_file() {
            return Ok((input.to_path_buf(), stem_base));
        }
        Err(EnviError::DataFileNotFound(
            input.display().to_string(),
            DATA_EXTENSIONS.join(", "),
        ))
    } else {
        // The common convention: replace the data file's own extension
        // with "hdr" (sample.dat -> sample.hdr). Falling back to
        // appending ".hdr" to the full filename (sample.dat.hdr) covers
        // the other real-world convention some tools use.
        let by_replacement = input.with_extension("hdr");
        if by_replacement.is_file() {
            return Ok((by_replacement, input.to_path_buf()));
        }
        let mut appended = input.as_os_str().to_os_string();
        appended.push(".hdr");
        let by_append = PathBuf::from(appended);
        if by_append.is_file() {
            return Ok((by_append, input.to_path_buf()));
        }
        Err(EnviError::HeaderNotFound(
            input.display().to_string(),
            by_replacement.display().to_string(),
        ))
    }
}

/// Reads one little/big-endian numeric element at ENVI data-type `code`
/// from `raw`, starting at byte `offset`, as `f32`.
fn decode_element(raw: &[u8], offset: usize, code: i64, big_endian: bool) -> Option<f32> {
    macro_rules! read {
        ($ty:ty, $size:expr) => {{
            let bytes: [u8; $size] = raw.get(offset..offset + $size)?.try_into().ok()?;
            Some(if big_endian {
                <$ty>::from_be_bytes(bytes) as f32
            } else {
                <$ty>::from_le_bytes(bytes) as f32
            })
        }};
    }
    match code {
        1 => raw.get(offset).map(|&b| b as f32),
        2 => read!(i16, 2),
        3 => read!(i32, 4),
        4 => read!(f32, 4),
        5 => read!(f64, 8),
        12 => read!(u16, 2),
        13 => read!(u32, 4),
        _ => None,
    }
}

fn element_size(code: i64) -> Option<usize> {
    match code {
        1 => Some(1),
        2 | 12 => Some(2),
        3 | 4 | 13 => Some(4),
        5 => Some(8),
        _ => None,
    }
}

/// Opens the ENVI header/data pair `input` points at (either file of
/// the pair) and returns the fully de-interleaved cube.
pub fn load_envi(input: impl AsRef<Path>) -> Result<EnviCube, EnviError> {
    let input = input.as_ref();
    let (header_path, data_path) = resolve_paths(input)?;
    let header_path_str = header_path.display().to_string();
    let data_path_str = data_path.display().to_string();

    let header_text = std::fs::read_to_string(&header_path).map_err(|source| EnviError::HeaderIo {
        path: header_path_str.clone(),
        source,
    })?;
    let entries = parse_header(&header_text);

    let samples = required_usize(&entries, "samples", &header_path_str)?;
    let lines = required_usize(&entries, "lines", &header_path_str)?;
    let bands = required_usize(&entries, "bands", &header_path_str)?;
    let header_offset = entries
        .get("header offset")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let data_type: i64 = entries
        .get("data type")
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| EnviError::MissingField {
            path: header_path_str.clone(),
            field: "data type".to_string(),
        })?;
    let big_endian = entries
        .get("byte order")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(0)
        == 1;
    let interleave = match entries.get("interleave").map(|s| s.to_lowercase()) {
        Some(ref s) if s == "bsq" => Interleave::Bsq,
        Some(ref s) if s == "bil" => Interleave::Bil,
        Some(ref s) if s == "bip" => Interleave::Bip,
        other => {
            return Err(EnviError::InvalidField {
                path: header_path_str,
                field: "interleave".to_string(),
                value: other.unwrap_or_default(),
                expected: "one of bsq/bil/bip".to_string(),
            })
        }
    };

    let elem_size = element_size(data_type).ok_or_else(|| EnviError::UnsupportedDataType {
        path: header_path_str.clone(),
        code: data_type,
    })?;

    let raw = std::fs::read(&data_path).map_err(|source| EnviError::DataIo {
        path: data_path_str.clone(),
        source,
    })?;
    let total = samples * lines * bands;
    let needed = header_offset + total * elem_size;
    if raw.len() < needed {
        return Err(EnviError::DataFileTooShort {
            path: data_path_str,
            expected: needed,
            found: raw.len(),
        });
    }

    let mut data = vec![0.0_f32; total];
    let mut offset = header_offset;
    let mut next = |raw: &[u8]| {
        let v = decode_element(raw, offset, data_type, big_endian).unwrap_or(0.0);
        offset += elem_size;
        v
    };
    match interleave {
        Interleave::Bsq => {
            for b in 0..bands {
                for l in 0..lines {
                    for s in 0..samples {
                        data[b * lines * samples + l * samples + s] = next(&raw);
                    }
                }
            }
        }
        Interleave::Bil => {
            for l in 0..lines {
                for b in 0..bands {
                    for s in 0..samples {
                        data[b * lines * samples + l * samples + s] = next(&raw);
                    }
                }
            }
        }
        Interleave::Bip => {
            for l in 0..lines {
                for s in 0..samples {
                    for b in 0..bands {
                        data[b * lines * samples + l * samples + s] = next(&raw);
                    }
                }
            }
        }
    }

    let wavelengths_nm = entries.get("wavelength").and_then(|raw| {
        let parsed: Option<Vec<f64>> = raw
            .split(',')
            .map(|tok| tok.trim().parse::<f64>().ok())
            .collect();
        parsed.filter(|v| v.len() == bands)
    });

    Ok(EnviCube {
        samples,
        lines,
        bands,
        wavelengths_nm,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "xenovision-envi-test-{label}-{}-{}",
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

    fn write_fixture(dir: &Path, interleave: &str, samples: usize, lines: usize, bands: usize) -> PathBuf {
        let dat_path = dir.join(format!("{interleave}.dat"));
        let hdr_path = dir.join(format!("{interleave}.hdr"));
        let value_at = |b: usize, l: usize, s: usize| (b * 100 + l * 10 + s) as f32;

        let mut bytes = Vec::new();
        match interleave {
            "bsq" => {
                for b in 0..bands {
                    for l in 0..lines {
                        for s in 0..samples {
                            bytes.extend_from_slice(&value_at(b, l, s).to_le_bytes());
                        }
                    }
                }
            }
            "bil" => {
                for l in 0..lines {
                    for b in 0..bands {
                        for s in 0..samples {
                            bytes.extend_from_slice(&value_at(b, l, s).to_le_bytes());
                        }
                    }
                }
            }
            "bip" => {
                for l in 0..lines {
                    for s in 0..samples {
                        for b in 0..bands {
                            bytes.extend_from_slice(&value_at(b, l, s).to_le_bytes());
                        }
                    }
                }
            }
            _ => unreachable!(),
        }
        std::fs::write(&dat_path, &bytes).unwrap();

        let wavelengths: Vec<String> = (0..bands)
            .map(|b| format!("{}", 400.0 + b as f64 * 50.0))
            .collect();
        let hdr = format!(
            "ENVI\nsamples = {samples}\nlines = {lines}\nbands = {bands}\nheader offset = 0\n\
             file type = ENVI Standard\ndata type = 4\ninterleave = {interleave}\nbyte order = 0\n\
             wavelength = {{{}}}\n",
            wavelengths.join(", ")
        );
        std::fs::write(&hdr_path, hdr).unwrap();
        dat_path
    }

    #[test]
    fn reads_bsq_bil_bip_identically_and_matches_hand_computed_values() {
        let dir = TempDir::new("interleaves");
        let (samples, lines, bands) = (4, 3, 5);
        for interleave in ["bsq", "bil", "bip"] {
            let path = write_fixture(&dir.0, interleave, samples, lines, bands);
            let cube = load_envi(&path).unwrap();
            assert_eq!((cube.samples, cube.lines, cube.bands), (samples, lines, bands));
            assert_eq!(
                cube.wavelengths_nm,
                Some(vec![400.0, 450.0, 500.0, 550.0, 600.0]),
                "interleave {interleave}"
            );
            for b in 0..bands {
                for l in 0..lines {
                    for s in 0..samples {
                        let expected = (b * 100 + l * 10 + s) as f32;
                        let actual = cube.data[b * lines * samples + l * samples + s];
                        assert_eq!(
                            actual, expected,
                            "interleave {interleave}, band {b} line {l} sample {s}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn multi_line_wavelength_list_parses_correctly() {
        let dir = TempDir::new("multiline");
        let dat_path = dir.0.join("sample.dat");
        let hdr_path = dir.0.join("sample.hdr");
        std::fs::write(&dat_path, [0u8; 4 * 3]).unwrap();
        let hdr = "ENVI\nsamples = 1\nlines = 1\nbands = 3\ndata type = 4\ninterleave = bsq\n\
                    byte order = 0\nwavelength = {\n 400.0,\n 410.0,\n 420.0}\n";
        std::fs::write(&hdr_path, hdr).unwrap();
        let cube = load_envi(&dat_path).unwrap();
        assert_eq!(cube.wavelengths_nm, Some(vec![400.0, 410.0, 420.0]));
    }

    #[test]
    fn wavelength_count_mismatch_is_none_not_misaligned() {
        let dir = TempDir::new("mismatch");
        let dat_path = dir.0.join("sample.dat");
        let hdr_path = dir.0.join("sample.hdr");
        std::fs::write(&dat_path, [0u8; 4 * 3]).unwrap();
        let hdr = "ENVI\nsamples = 1\nlines = 1\nbands = 3\ndata type = 4\ninterleave = bsq\n\
                    byte order = 0\nwavelength = {400.0, 410.0}\n";
        std::fs::write(&hdr_path, hdr).unwrap();
        let cube = load_envi(&dat_path).unwrap();
        assert_eq!(cube.wavelengths_nm, None);
    }

    #[test]
    fn unsupported_data_type_errors_with_the_code_named() {
        let dir = TempDir::new("unsupported");
        let dat_path = dir.0.join("sample.dat");
        let hdr_path = dir.0.join("sample.hdr");
        std::fs::write(&dat_path, [0u8; 100]).unwrap();
        let hdr = "ENVI\nsamples = 1\nlines = 1\nbands = 1\ndata type = 6\ninterleave = bsq\n";
        std::fs::write(&hdr_path, hdr).unwrap();
        let err = load_envi(&dat_path).unwrap_err();
        assert!(matches!(err, EnviError::UnsupportedDataType { code: 6, .. }));
    }

    #[test]
    fn pointing_at_the_header_file_also_works() {
        let dir = TempDir::new("via-header");
        let dat_path = write_fixture(&dir.0, "bsq", 2, 2, 2);
        let hdr_path = dat_path.with_extension("hdr");
        let cube = load_envi(&hdr_path).unwrap();
        assert_eq!((cube.samples, cube.lines, cube.bands), (2, 2, 2));
    }

    #[test]
    fn missing_header_is_a_clear_error_not_a_panic() {
        let dir = TempDir::new("missing");
        let dat_path = dir.0.join("sample.dat");
        std::fs::write(&dat_path, [0u8; 4]).unwrap();
        let err = load_envi(&dat_path).unwrap_err();
        assert!(matches!(err, EnviError::HeaderNotFound(..)));
    }

    #[test]
    fn truncated_data_file_errors_rather_than_reading_garbage() {
        let dir = TempDir::new("truncated");
        let dat_path = dir.0.join("sample.dat");
        let hdr_path = dir.0.join("sample.hdr");
        std::fs::write(&dat_path, [0u8; 4]).unwrap(); // only 1 float32, header claims 3
        let hdr = "ENVI\nsamples = 3\nlines = 1\nbands = 1\ndata type = 4\ninterleave = bsq\n";
        std::fs::write(&hdr_path, hdr).unwrap();
        let err = load_envi(&dat_path).unwrap_err();
        assert!(matches!(err, EnviError::DataFileTooShort { .. }));
    }
}
