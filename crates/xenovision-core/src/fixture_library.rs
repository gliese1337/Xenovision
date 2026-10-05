//! File-based packaging for the visual-system fixture library (design
//! doc §8.3): each fixture is its own user-editable JSON file in a
//! `fixtures` subdirectory of the app's per-user data directory
//! (`preset_store::data_dir()`) - not one combined array, so a fixture
//! can be added, hand-edited, or deleted independently of the rest.
//! Bootstrapped from `fixtures::builtin_fixtures()` on first run, with
//! `restore_builtin_defaults` as the recovery mechanism if a built-in
//! file gets corrupted or deleted.

use std::path::{Path, PathBuf};

use crate::curve_set::CurveSet;
use crate::fixtures::builtin_fixtures;
use crate::preset_store;

fn fixtures_dir() -> PathBuf {
    let dir = preset_store::data_dir().join("fixtures");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Turns a fixture's display name into a safe filename stem: lowercase
/// alphanumerics with runs of anything else collapsed to one underscore
/// (so e.g. "Frog (Rana spp.) — Scotopic" -> "frog_rana_spp_scotopic").
fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            pending_sep = false;
        } else {
            pending_sep = true;
        }
    }
    out
}

fn file_path_for(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.json", slug(name)))
}

fn read_all(dir: &Path) -> Vec<CurveSet> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|p| CurveSet::load_from_file(&p).ok())
        .collect()
}

fn write_fixture_in(dir: &Path, set: &CurveSet) -> std::io::Result<()> {
    let path = file_path_for(dir, &set.name);
    let json = serde_json::to_string_pretty(set).expect("CurveSet is always serializable");
    std::fs::write(path, json)
}

fn load_library_from(dir: &Path) -> Vec<CurveSet> {
    let sets = read_all(dir);
    if !sets.is_empty() {
        return sets;
    }
    for set in builtin_fixtures() {
        let _ = write_fixture_in(dir, &set);
    }
    read_all(dir)
}

fn restore_builtin_defaults_in(dir: &Path) -> Vec<CurveSet> {
    for set in builtin_fixtures() {
        let _ = write_fixture_in(dir, &set);
    }
    read_all(dir)
}

fn delete_fixture_in(dir: &Path, name: &str) -> std::io::Result<()> {
    let path = file_path_for(dir, name);
    if path.exists() {
        std::fs::remove_file(path)
    } else {
        Ok(())
    }
}

/// Loads the full on-disk fixture library, bootstrapping it from
/// `builtin_fixtures()` on first run (an empty/missing directory).
pub fn load_library() -> Vec<CurveSet> {
    load_library_from(&fixtures_dir())
}

/// Writes one fixture to its own file (named from a sanitized version of
/// its `name`), overwriting any existing file for that same name - used
/// both to persist edits to an existing entry and to save a new ("Save
/// As") entry.
pub fn write_fixture(set: &CurveSet) -> std::io::Result<()> {
    write_fixture_in(&fixtures_dir(), set)
}

/// Deletes the on-disk file for a fixture with this exact name, if any.
pub fn delete_fixture(name: &str) -> std::io::Result<()> {
    delete_fixture_in(&fixtures_dir(), name)
}

/// Overwrites the on-disk files for the 12 built-in fixtures back to
/// their pristine defaults (§8.3's recovery mechanism). Any
/// user-created fixtures saved under other names are left untouched.
/// Returns the resulting full on-disk library.
pub fn restore_builtin_defaults() -> Vec<CurveSet> {
    restore_builtin_defaults_in(&fixtures_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test gets its own scratch directory (not the shared
    /// `data_dir()`) so bootstrap-on-empty behavior can be tested
    /// without racing other tests or an actual user's library.
    fn scratch_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xenovision-fixture-library-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cleanup(dir: &Path) {
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn slug_collapses_punctuation_and_lowercases() {
        assert_eq!(
            slug("Dog (Canis lupus familiaris)"),
            "dog_canis_lupus_familiaris"
        );
        assert_eq!(
            slug("Frog (Rana spp.) — Scotopic"),
            "frog_rana_spp_scotopic"
        );
    }

    #[test]
    fn first_run_bootstraps_all_12_builtin_fixtures() {
        let dir = scratch_dir("bootstrap");
        let loaded = load_library_from(&dir);
        assert_eq!(loaded.len(), 12);
        let mut names: Vec<&str> = loaded.iter().map(|s| s.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(
            names.len(),
            12,
            "every bootstrapped fixture must be distinct"
        );

        // Loading again should read the now-populated directory back,
        // not re-bootstrap (and still find the same 12 fixtures).
        let loaded_again = load_library_from(&dir);
        assert_eq!(loaded_again.len(), 12);

        cleanup(&dir);
    }

    #[test]
    fn write_fixture_persists_and_round_trips() {
        let dir = scratch_dir("write");
        let mut set = CurveSet::new("Test Species (Testus testicus)");
        set.colorspace_curves.push(crate::curve::SpectralCurve::new(
            "X-cone",
            crate::curve::CurveType::Sensitivity,
        ));
        write_fixture_in(&dir, &set).unwrap();

        let loaded = read_all(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], set);

        cleanup(&dir);
    }

    #[test]
    fn write_fixture_overwrites_same_name_rather_than_duplicating() {
        let dir = scratch_dir("overwrite");
        let mut set = CurveSet::new("Test Species (Testus testicus)");
        write_fixture_in(&dir, &set).unwrap();
        set.metadata.insert("edited".to_string(), "yes".to_string());
        write_fixture_in(&dir, &set).unwrap();

        let loaded = read_all(&dir);
        assert_eq!(loaded.len(), 1, "same name must overwrite, not duplicate");
        assert_eq!(
            loaded[0].metadata.get("edited").map(String::as_str),
            Some("yes")
        );

        cleanup(&dir);
    }

    #[test]
    fn restore_builtin_defaults_overwrites_edited_builtin_but_keeps_custom_fixtures() {
        let dir = scratch_dir("restore");
        load_library_from(&dir); // bootstrap the 12 built-ins

        // Simulate a user edit to a built-in fixture's on-disk file...
        let mut edited_human = crate::fixtures::human();
        edited_human
            .metadata
            .insert("user_edit".to_string(), "oops".to_string());
        write_fixture_in(&dir, &edited_human).unwrap();

        // ...and a brand-new custom fixture saved under its own name.
        let custom = CurveSet::new("My Custom System");
        write_fixture_in(&dir, &custom).unwrap();

        let restored = restore_builtin_defaults_in(&dir);
        assert_eq!(restored.len(), 13, "12 restored built-ins + 1 custom");

        let human_restored = restored
            .iter()
            .find(|s| s.name == "Human (Homo sapiens)")
            .unwrap();
        assert!(
            !human_restored.metadata.contains_key("user_edit"),
            "built-in should be back to pristine"
        );
        assert!(
            restored.iter().any(|s| s.name == "My Custom System"),
            "custom fixture must survive a restore"
        );

        cleanup(&dir);
    }

    #[test]
    fn delete_fixture_removes_its_file() {
        let dir = scratch_dir("delete");
        let set = CurveSet::new("Deletable System");
        write_fixture_in(&dir, &set).unwrap();
        assert_eq!(read_all(&dir).len(), 1);

        delete_fixture_in(&dir, "Deletable System").unwrap();
        assert_eq!(read_all(&dir).len(), 0);

        // Deleting an already-absent fixture is not an error.
        delete_fixture_in(&dir, "Deletable System").unwrap();

        cleanup(&dir);
    }
}
