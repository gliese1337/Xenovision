//! Shared library of loaded hyperspectral/multispectral image cubes.
//!
//! Both the Stimulus Editor (which extracts stimulus curves from a region
//! of an image) and the Image Display window (which renders an image's
//! pixels through a visual system's receptors) draw from this one list of
//! loaded sources, so a file is loaded at most once and its named regions
//! are shared. A source owns its cube, a monotonically increasing
//! `generation` (bumped by any in-place edit, which is the cache key both
//! panels use to notice the cube changed under them), its currently
//! previewed band, and any regions the user has saved on it.
//!
//! Unlike a visual system's own Sensitivity curves, an image source is a
//! transient working set: it is not persisted to disk and nothing else
//! references it by id across a save/load.

use std::sync::Arc;

use eframe::egui;
use xenovision_core::hyperspectral::{HyperspectralCube, Region};

/// Stable id for one loaded image source (never reused within a session).
pub type ImageSourceId = u64;

/// A user-named region saved on an image source, for reuse by either
/// panel ("whole image" is represented by `Option::None`, not a saved
/// region).
#[derive(Clone)]
pub struct NamedRegion {
    pub name: String,
    pub region: Region,
}

/// One loaded image cube plus the view state that travels with it.
pub struct ImageSource {
    pub id: ImageSourceId,
    pub label: String,
    pub path: String,
    /// The cube behind an `Arc` so a background render can borrow it
    /// (shared, cheap to clone the handle) without the UI thread copying
    /// the pixel data; assigning a wavelength axis copy-on-writes it.
    pub cube: Arc<HyperspectralCube>,
    /// Bumped by any in-place edit to `cube`; both panels' render caches
    /// key on it (alongside the source id) so a re-assigned wavelength
    /// axis invalidates stale renders.
    pub generation: usize,
    /// The band shown in the Stimulus Editor's grayscale preview.
    pub band: usize,
    /// Cached grayscale band preview (Stimulus Editor) and the
    /// `(generation, band)` it was built from.
    pub preview: Option<egui::TextureHandle>,
    pub preview_key: Option<(usize, usize)>,
    pub regions: Vec<NamedRegion>,
}

impl ImageSource {
    pub fn new(label: impl Into<String>, path: impl Into<String>, cube: HyperspectralCube) -> Self {
        ImageSource {
            id: 0, // assigned by `ImageLibrary::add`
            label: label.into(),
            path: path.into(),
            cube: Arc::new(cube),
            generation: 0,
            band: 0,
            preview: None,
            preview_key: None,
            regions: Vec::new(),
        }
    }

    /// Records that the cube was edited in place (e.g. a wavelength axis
    /// was assigned): invalidates the band preview and every render cache
    /// keyed on this source's generation.
    pub fn bump_generation(&mut self) {
        self.generation += 1;
        self.preview = None;
        self.preview_key = None;
    }

    /// Saves `region` under `name`, replacing an existing region of the
    /// same name (so re-selecting doesn't accumulate duplicates).
    pub fn save_region(&mut self, name: impl Into<String>, region: Region) {
        let name = name.into();
        match self.regions.iter_mut().find(|r| r.name == name) {
            Some(existing) => existing.region = region,
            None => self.regions.push(NamedRegion { name, region }),
        }
    }
}

/// All loaded image sources; `selected` is the one the Stimulus Editor's
/// image-import pane is currently pointed at (the Image Display selects
/// per-view instead).
#[derive(Default)]
pub struct ImageLibrary {
    pub sources: Vec<ImageSource>,
    selected: Option<ImageSourceId>,
    next_id: ImageSourceId,
}

impl ImageLibrary {
    /// Adds `source`, assigning it an id and selecting it. Returns the id.
    pub fn add(&mut self, mut source: ImageSource) -> ImageSourceId {
        let id = self.next_id;
        self.next_id += 1;
        source.id = id;
        self.sources.push(source);
        self.selected = Some(id);
        id
    }

    pub fn get(&self, id: ImageSourceId) -> Option<&ImageSource> {
        self.sources.iter().find(|s| s.id == id)
    }

    pub fn get_mut(&mut self, id: ImageSourceId) -> Option<&mut ImageSource> {
        self.sources.iter_mut().find(|s| s.id == id)
    }

    pub fn selected_id(&self) -> Option<ImageSourceId> {
        self.selected
    }

    pub fn select(&mut self, id: ImageSourceId) {
        if self.get(id).is_some() {
            self.selected = Some(id);
        }
    }

    pub fn selected(&self) -> Option<&ImageSource> {
        self.selected.and_then(|id| self.get(id))
    }

    /// Removes a source (and clears `selected` if it was the selected
    /// one). Named-region / view references to it simply stop resolving.
    pub fn remove(&mut self, id: ImageSourceId) {
        self.sources.retain(|s| s.id != id);
        if self.selected == Some(id) {
            self.selected = self.sources.first().map(|s| s.id);
        }
    }
}

/// A short, human-readable label for a region - the same text the
/// Stimulus Editor has always used in the extracted curve's name and the
/// Image Display shows in a view's provenance bar.
pub fn region_label(region: &Region) -> String {
    match region {
        Region::Rectangle { x0, y0, x1, y1 } => {
            format!("rect ({x0:.0},{y0:.0})-({x1:.0},{y1:.0})")
        }
        Region::Polygon(points) => format!("polygon ({} vertices)", points.len()),
    }
}

/// A source's full provenance string ("label · N bands @ first–last nm"),
/// used wherever an image has to identify itself.
pub fn source_label(source: &ImageSource) -> String {
    match &source.cube.wavelengths_nm {
        Some(wl) if !wl.is_empty() => format!(
            "{} · {} bands @ {:.0}–{:.0} nm",
            source.label,
            source.cube.bands,
            wl.first().copied().unwrap_or(0.0),
            wl.last().copied().unwrap_or(0.0),
        ),
        _ => format!(
            "{} · {} bands (no wavelength axis)",
            source.label, source.cube.bands
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xenovision_core::hyperspectral::HyperspectralCube;

    fn cube() -> HyperspectralCube {
        HyperspectralCube::from_band_major(2, 2, 1, Some(vec![550.0]), vec![1.0; 4], "c")
    }

    #[test]
    fn add_selects_and_remove_falls_back() {
        let mut lib = ImageLibrary::default();
        let a = lib.add(ImageSource::new("a", "a", cube()));
        let b = lib.add(ImageSource::new("b", "b", cube()));
        // The most recently added source is selected.
        assert_eq!(lib.selected_id(), Some(b));
        assert_eq!(lib.selected().unwrap().label, "b");
        lib.select(a);
        assert_eq!(lib.selected().unwrap().label, "a");
        lib.remove(a);
        // Removing the selected source falls back to the first remaining.
        assert_eq!(lib.selected_id(), Some(b));
        assert!(lib.get(a).is_none());
    }

    #[test]
    fn save_region_replaces_same_name() {
        let mut src = ImageSource::new("s", "s", cube());
        src.save_region(
            "r",
            Region::Rectangle {
                x0: 0.0,
                y0: 0.0,
                x1: 1.0,
                y1: 1.0,
            },
        );
        src.save_region(
            "r",
            Region::Rectangle {
                x0: 0.0,
                y0: 0.0,
                x1: 2.0,
                y1: 2.0,
            },
        );
        assert_eq!(src.regions.len(), 1, "same-named region should replace");
        assert!(matches!(
            src.regions[0].region,
            Region::Rectangle { x1, .. } if x1 == 2.0
        ));
    }

    #[test]
    fn bump_generation_invalidates_preview() {
        let mut src = ImageSource::new("s", "s", cube());
        src.preview_key = Some((0, 0));
        let before = src.generation;
        src.bump_generation();
        assert_eq!(src.generation, before + 1);
        assert!(src.preview_key.is_none());
    }

    #[test]
    fn source_label_reports_wavelength_span() {
        let src = ImageSource::new("img", "img", cube());
        let label = source_label(&src);
        assert!(label.contains("img"));
        assert!(label.contains("550"), "{label}");
    }
}
