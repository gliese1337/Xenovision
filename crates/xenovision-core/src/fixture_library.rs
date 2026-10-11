//! File-based packaging for the visual-system fixture library (design
//! doc §8.3): each fixture is its own user-editable JSON file in a
//! `fixtures` subdirectory of the app's per-user data directory
//! (`preset_store::data_dir()`) - not one combined array, so a fixture
//! can be added, hand-edited, or deleted independently of the rest.
//! Bootstrapped from `fixtures::builtin_fixtures()` on first run. On every
//! load the built-ins are also *reconciled* against the shipped
//! definitions: a built-in file the user has never edited is refreshed
//! when the shipped definition has moved on, while edited files and
//! custom fixtures are left alone. `restore_builtin_defaults` remains the
//! explicit recovery mechanism if a built-in file gets corrupted or
//! deleted.

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

/// Metadata key stamped into every file the app writes from a built-in
/// definition: the content hash of that definition *excluding this key*.
/// It lets reconciliation tell an unedited built-in snapshot (marker
/// present and matching) from user content (marker absent or mismatched,
/// or a name that isn't a built-in) - only the former is ever refreshed
/// to a newer shipped definition.
const BUILTIN_HASH_KEY: &str = "builtin_hash";

/// Stable 64-bit FNV-1a hash of a fixture's canonical JSON with the
/// built-in marker removed. Unlike `DefaultHasher` this is stable across
/// releases, so a marker written by an earlier build still validates.
fn content_hash(set: &CurveSet) -> u64 {
    let mut probe = set.clone();
    probe.metadata.remove(BUILTIN_HASH_KEY);
    let json = serde_json::to_string(&probe).expect("CurveSet is always serializable");
    json.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The marker string for `set`'s current content.
fn builtin_marker(set: &CurveSet) -> String {
    format!("{:016x}", content_hash(set))
}

/// A built-in fixture as written to disk: `set` with its content marker
/// stamped into the metadata.
fn stamped_builtin(set: &CurveSet) -> CurveSet {
    let mut out = set.clone();
    out.metadata
        .insert(BUILTIN_HASH_KEY.to_string(), builtin_marker(set));
    out
}

/// Whether `set`, as loaded from disk, is an unedited snapshot of some
/// built-in definition (its marker is present and still matches its own
/// content). User edits break the match and are therefore preserved.
fn is_unedited_builtin(set: &CurveSet) -> bool {
    set.metadata
        .get(BUILTIN_HASH_KEY)
        .is_some_and(|stored| *stored == builtin_marker(set))
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

/// Refreshes any present built-in file that is still an unedited snapshot
/// of a *previous* shipped definition, leaving user-edited and custom
/// files untouched. A built-in the user deleted stays deleted.
fn reconcile_builtins_in(dir: &Path, sets: &mut [CurveSet]) {
    for builtin in builtin_fixtures() {
        let Some(disk) = sets.iter_mut().find(|s| s.name == builtin.name) else {
            continue;
        };
        if is_unedited_builtin(disk) && content_hash(disk) != content_hash(&builtin) {
            *disk = stamped_builtin(&builtin);
            let _ = write_fixture_in(dir, disk);
        }
    }
}

fn load_library_from(dir: &Path) -> Vec<CurveSet> {
    let mut sets = read_all(dir);
    if sets.is_empty() {
        // First run (empty/missing directory): extract every built-in.
        for set in builtin_fixtures() {
            let _ = write_fixture_in(dir, &stamped_builtin(&set));
        }
        return read_all(dir);
    }
    reconcile_builtins_in(dir, &mut sets);
    sets
}

fn restore_builtin_defaults_in(dir: &Path) -> Vec<CurveSet> {
    for set in builtin_fixtures() {
        let _ = write_fixture_in(dir, &stamped_builtin(&set));
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

/// Overwrites the on-disk files for the 11 built-in fixtures back to
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
        assert_eq!(slug("Frog (Rana spp.)"), "frog_rana_spp");
    }

    #[test]
    fn first_run_bootstraps_all_11_builtin_fixtures() {
        let dir = scratch_dir("bootstrap");
        let loaded = load_library_from(&dir);
        assert_eq!(loaded.len(), 11, "frog's two files are now one");
        let mut names: Vec<&str> = loaded.iter().map(|s| s.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(
            names.len(),
            11,
            "every bootstrapped fixture must be distinct"
        );

        // Loading again should read the now-populated directory back,
        // not re-bootstrap (and still find the same 11 fixtures).
        let loaded_again = load_library_from(&dir);
        assert_eq!(loaded_again.len(), 11);

        cleanup(&dir);
    }

    #[test]
    fn write_fixture_persists_and_round_trips() {
        let dir = scratch_dir("write");
        let mut set = CurveSet::new("Test Species (Testus testicus)");
        set.curves.push(crate::curve::SpectralCurve::new(
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
        load_library_from(&dir); // bootstrap the 11 built-ins

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
        assert_eq!(restored.len(), 12, "11 restored built-ins + 1 custom");

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

    #[test]
    fn bootstrapped_builtins_are_stamped_and_idempotent() {
        let dir = scratch_dir("stamp");
        let loaded = load_library_from(&dir);
        assert_eq!(loaded.len(), 11);
        assert!(
            loaded.iter().all(is_unedited_builtin),
            "every bootstrapped built-in must carry a matching marker"
        );
        assert_eq!(
            load_library_from(&dir),
            loaded,
            "a second load with unchanged built-ins is a no-op"
        );
        cleanup(&dir);
    }

    #[test]
    fn unedited_builtin_is_refreshed_when_the_shipped_definition_changes() {
        let dir = scratch_dir("refresh");

        // A previous release's built-in: the real fixture with one extra
        // field, stamped so it reads as a genuine unedited snapshot.
        let mut old_human = crate::fixtures::human();
        old_human
            .metadata
            .insert("era".to_string(), "previous".to_string());
        write_fixture_in(&dir, &stamped_builtin(&old_human)).unwrap();
        // A custom fixture must survive untouched.
        write_fixture_in(&dir, &CurveSet::new("My Custom System")).unwrap();

        let loaded = load_library_from(&dir);
        let human = loaded
            .iter()
            .find(|s| s.name == "Human (Homo sapiens)")
            .unwrap();
        assert!(
            !human.metadata.contains_key("era"),
            "an unedited built-in must be refreshed to the shipped definition"
        );
        assert!(
            loaded.iter().any(|s| s.name == "My Custom System"),
            "custom fixtures must survive reconciliation"
        );
        cleanup(&dir);
    }

    #[test]
    fn edited_or_tampered_builtin_is_preserved() {
        // An unmarked file (as a user save would leave it) ...
        let dir = scratch_dir("preserve-unmarked");
        let mut edited = crate::fixtures::human();
        edited
            .metadata
            .insert("user_edit".to_string(), "keep me".to_string());
        write_fixture_in(&dir, &edited).unwrap();
        let loaded = load_library_from(&dir);
        let human = loaded
            .iter()
            .find(|s| s.name == "Human (Homo sapiens)")
            .unwrap();
        assert_eq!(
            human.metadata.get("user_edit").map(String::as_str),
            Some("keep me"),
            "a user-edited built-in must not be overwritten"
        );
        cleanup(&dir);

        // ... and a stamped file whose content later diverged from its
        // marker (an edit made after the snapshot was written).
        let dir = scratch_dir("preserve-tampered");
        let mut tampered = stamped_builtin(&crate::fixtures::human());
        tampered
            .metadata
            .insert("user_edit".to_string(), "keep me too".to_string());
        write_fixture_in(&dir, &tampered).unwrap();
        let loaded = load_library_from(&dir);
        let human = loaded
            .iter()
            .find(|s| s.name == "Human (Homo sapiens)")
            .unwrap();
        assert_eq!(
            human.metadata.get("user_edit").map(String::as_str),
            Some("keep me too"),
            "a built-in whose marker no longer matches is user content"
        );
        cleanup(&dir);
    }

    #[test]
    fn deleted_builtin_stays_deleted() {
        let dir = scratch_dir("deleted");
        load_library_from(&dir); // bootstrap the 11 built-ins
        delete_fixture_in(&dir, "Human (Homo sapiens)").unwrap();

        let loaded = load_library_from(&dir);
        assert_eq!(loaded.len(), 10, "a deleted built-in is not resurrected");
        assert!(
            !loaded.iter().any(|s| s.name == "Human (Homo sapiens)"),
            "reconciliation must respect a user's deletion"
        );
        cleanup(&dir);
    }
}
