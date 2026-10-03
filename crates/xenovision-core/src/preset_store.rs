//! Generic load/save for user-editable preset libraries (illuminant
//! notch/narrow-band-source presets for now; Phase 6/8's species fixture
//! files will reuse the same `data_dir` convention per §8.3).
//!
//! Presets are stored as plain JSON files in the OS-appropriate per-user
//! data directory, editable by hand outside the app too (consistent with
//! §8.3's "ordinary, user-editable files - not read-only bundled
//! resources").

use std::path::PathBuf;

use directories::ProjectDirs;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// The app's per-user data directory, creating it if it doesn't exist
/// yet. Falls back to the current working directory if the OS-level
/// lookup fails for some reason (no `HOME`/equivalent set) - better to
/// write presets somewhere than to make the generators unusable because
/// of an unrelated environment quirk.
pub fn data_dir() -> PathBuf {
    let dir = ProjectDirs::from("", "", "xenovision")
        .map(|p| p.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Reads `filename` from `data_dir()` and parses it as JSON. On *any*
/// failure (file missing, unreadable, or corrupt) - never propagates an
/// error to the caller: a broken preset file is a non-critical problem
/// and shouldn't be able to block someone from using the generators, so
/// this writes and returns `builtin_defaults()` instead.
pub fn load_or_init<T: Serialize + DeserializeOwned + Clone>(
    filename: &str,
    builtin_defaults: impl Fn() -> Vec<T>,
) -> Vec<T> {
    let path = data_dir().join(filename);
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(parsed) = serde_json::from_str::<Vec<T>>(&text) {
            return parsed;
        }
    }
    let defaults = builtin_defaults();
    let _ = save(filename, &defaults);
    defaults
}

/// Writes `items` to `filename` in `data_dir()` as pretty JSON.
pub fn save<T: Serialize>(filename: &str, items: &[T]) -> std::io::Result<()> {
    let path = data_dir().join(filename);
    let json = serde_json::to_string_pretty(items).expect("preset items are always serializable");
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Dummy {
        name: String,
        value: f64,
    }

    fn defaults() -> Vec<Dummy> {
        vec![
            Dummy {
                name: "a".to_string(),
                value: 1.0,
            },
            Dummy {
                name: "b".to_string(),
                value: 2.0,
            },
        ]
    }

    /// Isolates each test to its own filename so parallel test execution
    /// against the real (shared) data_dir() doesn't race.
    fn unique_filename(label: &str) -> String {
        format!(
            "test-{label}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[test]
    fn missing_file_writes_and_returns_defaults() {
        let filename = unique_filename("missing");
        let path = data_dir().join(&filename);
        assert!(!path.exists());

        let loaded = load_or_init(&filename, defaults);
        assert_eq!(loaded, defaults());
        assert!(path.exists(), "should have written the defaults out");

        // And loading again should now read that same file back.
        let loaded_again = load_or_init(&filename, defaults);
        assert_eq!(loaded_again, defaults());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults_without_panicking() {
        let filename = unique_filename("corrupt");
        let path = data_dir().join(&filename);
        std::fs::write(&path, "{ not valid json at all").unwrap();

        let loaded = load_or_init(&filename, defaults);
        assert_eq!(loaded, defaults());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn valid_file_round_trips() {
        let filename = unique_filename("roundtrip");
        let custom = vec![Dummy {
            name: "custom".to_string(),
            value: 42.0,
        }];
        save(&filename, &custom).unwrap();

        let loaded = load_or_init(&filename, defaults);
        assert_eq!(
            loaded, custom,
            "should load what was saved, not the builtin defaults"
        );

        std::fs::remove_file(data_dir().join(&filename)).ok();
    }
}
