//! Groups the shared stimulus library (`AppState::stimulus_curves`) into
//! a hierarchy of corpora/sub-corpora, the thing that lets a bulk import
//! (e.g. "every pixel in this image", §4.2.6) stay usable in the UI
//! instead of rendering one row per curve, and lets related corpora
//! (e.g. several regions of the same image, or several images of the
//! same scene type) be bulk-selected together for opponency calculations
//! or radiance-to-reflectance conversion without hand-picking every
//! curve inside them.
//!
//! A curve's place in the hierarchy is its `metadata["batch"]` value,
//! read as a `/`-separated path (e.g. `"forest.tif/whole image"` nests
//! under a `forest.tif` corpus); a curve with no `"batch"` entry is
//! ungrouped and appears as its own row, same as before this existed.
//! Editable directly as free text (the Stimulus Editor's "Corpus" field
//! on a curve), or assigned automatically by a bulk import/conversion
//! tool.

use eframe::egui;

use crate::state::{AppState, CurveId};

/// One node in the corpus tree: either a named corpus/sub-corpus
/// (`is_named_group`, holding zero or more curves directly plus zero or
/// more child corpora), or the single-curve pseudo-node standing in for
/// an ungrouped curve (`!is_named_group`, always exactly one entry in
/// `curves`, no children). Curve names are captured here (rather than
/// looked up from `AppState` again) so rendering doesn't need to borrow
/// the app a second time.
pub struct CorpusNode {
    pub name: String,
    pub is_named_group: bool,
    pub children: Vec<CorpusNode>,
    pub curves: Vec<(CurveId, String)>,
}

impl CorpusNode {
    /// Every curve under this node: its own direct members plus every
    /// descendant's.
    pub fn all_curves(&self) -> Vec<CurveId> {
        let mut out: Vec<CurveId> = self.curves.iter().map(|(id, _)| *id).collect();
        for child in &self.children {
            out.extend(child.all_curves());
        }
        out
    }
}

/// A curve's corpus path, split on `/` and trimmed - empty if the curve
/// has no `"batch"` metadata (or it's blank).
fn corpus_path(metadata: &std::collections::BTreeMap<String, String>) -> Vec<String> {
    metadata
        .get("batch")
        .map(|b| {
            b.split('/')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn insert_into_tree(nodes: &mut Vec<CorpusNode>, path: &[String], id: CurveId, name: &str) {
    let head = &path[0];
    let idx = match nodes.iter().position(|n| &n.name == head) {
        Some(i) => i,
        None => {
            nodes.push(CorpusNode {
                name: head.clone(),
                is_named_group: true,
                children: Vec::new(),
                curves: Vec::new(),
            });
            nodes.len() - 1
        }
    };
    if path.len() == 1 {
        nodes[idx].curves.push((id, name.to_string()));
    } else {
        insert_into_tree(&mut nodes[idx].children, &path[1..], id, name);
    }
}

/// Builds the corpus tree from `app.stimulus_order`, keeping only
/// curves `filter` accepts (e.g. excluding Absorption curves, or
/// restricting to Radiance curves for the bulk radiance->reflectance
/// picker) - a node with nothing left under it after filtering is
/// dropped entirely.
pub fn corpus_tree(
    app: &AppState,
    filter: &dyn Fn(&xenovision_core::SpectralCurve) -> bool,
) -> Vec<CorpusNode> {
    let mut roots: Vec<CorpusNode> = Vec::new();
    for &id in &app.stimulus_order {
        let Some(entry) = app.stimulus_curves.get(&id) else {
            continue;
        };
        if !filter(&entry.curve) {
            continue;
        }
        let path = corpus_path(&entry.curve.metadata);
        if path.is_empty() {
            roots.push(CorpusNode {
                name: entry.curve.name.clone(),
                is_named_group: false,
                children: Vec::new(),
                curves: vec![(id, entry.curve.name.clone())],
            });
        } else {
            insert_into_tree(&mut roots, &path, id, &entry.curve.name);
        }
    }
    roots
}

/// Renders `tree` as a checklist: a `CollapsingHeader` per named corpus
/// (with its own "select everything under this corpus" checkbox,
/// checked iff every curve beneath it is currently included) nesting
/// sub-corpora and direct members, and a plain checkbox per ungrouped
/// curve - the pre-hierarchy behavior, unchanged for anyone not using
/// corpora. `is_included`/`on_toggle` are the selection-state hook, so
/// one renderer serves every picker regardless of what it selects into
/// (a `Vec<CurveId>`, a `HashSet`, or a weighted `HashMap`).
pub fn render_corpus_checklist(
    ui: &mut egui::Ui,
    tree: &[CorpusNode],
    is_included: &dyn Fn(CurveId) -> bool,
    on_toggle: &mut dyn FnMut(&[CurveId], bool),
) {
    for node in tree {
        render_corpus_node(ui, node, is_included, on_toggle);
    }
}

fn render_corpus_node(
    ui: &mut egui::Ui,
    node: &CorpusNode,
    is_included: &dyn Fn(CurveId) -> bool,
    on_toggle: &mut dyn FnMut(&[CurveId], bool),
) {
    if !node.is_named_group {
        let (id, name) = &node.curves[0];
        let mut included = is_included(*id);
        if ui.checkbox(&mut included, name).changed() {
            on_toggle(&[*id], included);
        }
        return;
    }

    let all_ids = node.all_curves();
    let mut whole_subtree_included = !all_ids.is_empty() && all_ids.iter().all(|&id| is_included(id));
    let plural = if all_ids.len() == 1 { "" } else { "s" };
    let header = format!("{} ({} curve{plural})", node.name, all_ids.len());

    ui.horizontal(|ui| {
        if ui.checkbox(&mut whole_subtree_included, "").changed() {
            on_toggle(&all_ids, whole_subtree_included);
        }
        ui.collapsing(header, |ui| {
            for (id, name) in &node.curves {
                let mut included = is_included(*id);
                if ui.checkbox(&mut included, name).changed() {
                    on_toggle(&[*id], included);
                }
            }
            for child in &node.children {
                render_corpus_node(ui, child, is_included, on_toggle);
            }
        });
    });
}

/// Gets/sets a curve's corpus path directly as the raw `"batch"`
/// metadata string (e.g. `"forest.tif/whole image"`) - the Stimulus
/// Editor's manual corpus-assignment field. Setting it to blank removes
/// the metadata entry entirely, same as never having set it.
pub fn corpus_path_field(
    ui: &mut egui::Ui,
    metadata: &mut std::collections::BTreeMap<String, String>,
) -> bool {
    let mut buf = metadata.get("batch").cloned().unwrap_or_default();
    let changed = ui.text_edit_singleline(&mut buf).changed();
    if changed {
        if buf.trim().is_empty() {
            metadata.remove("batch");
        } else {
            metadata.insert("batch".to_string(), buf);
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StimulusEntry;
    use xenovision_core::{CurveType, QuantityKind, SpectralCurve};

    fn curve_with_batch(name: &str, batch: Option<&str>) -> SpectralCurve {
        let mut c = SpectralCurve::new(name, CurveType::Reflectance)
            .with_points(vec![(400.0, 0.5), (700.0, 0.5)])
            .with_quantity(QuantityKind::Reflectance);
        if let Some(b) = batch {
            c.metadata.insert("batch".to_string(), b.to_string());
        }
        c
    }

    fn app_with_curves(curves: Vec<SpectralCurve>) -> AppState {
        let mut app = AppState::new();
        // AppState::new() seeds a few example stimuli so Comparison has
        // something to work with immediately - cleared here so these
        // tests see exactly the curves they add.
        app.stimulus_curves.clear();
        app.stimulus_order.clear();
        for curve in curves {
            let id = app.alloc_id();
            app.stimulus_order.push(id);
            app.stimulus_curves.insert(
                id,
                StimulusEntry {
                    curve,
                    file_path: None,
                    dirty: false,
                },
            );
        }
        app
    }

    fn accept_all(_: &SpectralCurve) -> bool {
        true
    }

    #[test]
    fn ungrouped_curves_are_singleton_nodes() {
        let app = app_with_curves(vec![curve_with_batch("a", None), curve_with_batch("b", None)]);
        let tree = corpus_tree(&app, &accept_all);
        assert_eq!(tree.len(), 2);
        assert!(tree.iter().all(|n| !n.is_named_group));
        assert!(tree.iter().all(|n| n.children.is_empty()));
        assert!(tree.iter().all(|n| n.curves.len() == 1));
    }

    #[test]
    fn single_level_batch_groups_curves_under_one_named_node() {
        let app = app_with_curves(vec![
            curve_with_batch("a", Some("Forest")),
            curve_with_batch("b", Some("Forest")),
            curve_with_batch("c", None),
        ]);
        let tree = corpus_tree(&app, &accept_all);
        assert_eq!(tree.len(), 2, "Forest group + ungrouped c");
        let forest = tree.iter().find(|n| n.name == "Forest").unwrap();
        assert!(forest.is_named_group);
        assert_eq!(forest.curves.len(), 2);
        assert_eq!(forest.all_curves().len(), 2);
    }

    #[test]
    fn nested_path_builds_a_real_hierarchy() {
        let app = app_with_curves(vec![
            curve_with_batch("a", Some("forest.tif/Canopy")),
            curve_with_batch("b", Some("forest.tif/Understory")),
            curve_with_batch("c", Some("forest.tif/Canopy")),
        ]);
        let tree = corpus_tree(&app, &accept_all);
        assert_eq!(tree.len(), 1);
        let root = &tree[0];
        assert_eq!(root.name, "forest.tif");
        assert!(root.curves.is_empty(), "no curves directly on the root");
        assert_eq!(root.children.len(), 2);
        let canopy = root.children.iter().find(|n| n.name == "Canopy").unwrap();
        assert_eq!(canopy.curves.len(), 2);
        let understory = root
            .children
            .iter()
            .find(|n| n.name == "Understory")
            .unwrap();
        assert_eq!(understory.curves.len(), 1);
        assert_eq!(root.all_curves().len(), 3);
    }

    #[test]
    fn a_node_can_have_both_direct_members_and_children() {
        let app = app_with_curves(vec![
            curve_with_batch("a", Some("forest.tif")),
            curve_with_batch("b", Some("forest.tif/Canopy")),
        ]);
        let tree = corpus_tree(&app, &accept_all);
        assert_eq!(tree.len(), 1);
        let root = &tree[0];
        assert_eq!(root.curves.len(), 1);
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.all_curves().len(), 2);
    }

    #[test]
    fn filter_excludes_curves_and_drops_empty_nodes() {
        let app = app_with_curves(vec![
            curve_with_batch("a", Some("Forest")),
            {
                let mut c = curve_with_batch("absorb", Some("Forest"));
                c.quantity = QuantityKind::Absorption;
                c
            },
        ]);
        let not_absorption = |c: &SpectralCurve| c.quantity != QuantityKind::Absorption;
        let tree = corpus_tree(&app, &not_absorption);
        let forest = tree.iter().find(|n| n.name == "Forest").unwrap();
        assert_eq!(forest.curves.len(), 1);
        assert_eq!(forest.curves[0].1, "a");
    }

    #[test]
    fn blank_batch_segments_are_ignored() {
        let app = app_with_curves(vec![curve_with_batch("a", Some("Forest//Canopy/"))]);
        let tree = corpus_tree(&app, &accept_all);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].name, "Forest");
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].name, "Canopy");
        assert_eq!(tree[0].children[0].curves.len(), 1);
    }
}
