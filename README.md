# Xenovision User Guide

A reference to every window, menu, and gesture in the current (three-window)
build. See `docs/design-doc.md` if you want the modeling math.

## Overview

Xenovision models and compares how different species perceive color. You
define a **visual system** as a set of photoreceptor sensitivity curves,
Xenovision derives that system's luminance and chroma channels from them, and
then evaluates arbitrary **stimulus spectra** (reflectance samples, light
sources) through it to get perceptual coordinates and distances. Twelve
literature-based species are built in, from trichromatic humans to the
twelve-receptor mantis shrimp, and you can define your own alongside them.

The interface is three panels, each shown as a tab in the main window or
in its own separate window (see [Arranging windows](#arranging-windows)):

- **Workspace** - define and edit visual systems: receptor curves, noise
  data, opponent contrasts. Always open.
- **Comparison** - run stimuli through one or more visual systems and read
  off coordinates, distances, and cross-species summaries. Opens on demand.
- **Stimulus Editor** - create and edit the stimulus curves (reflectance or
  radiance) that Comparison evaluates - by hand, derived from another curve,
  or imported from an image or a text/CSV file. Opens on demand.

All three read and write the same underlying data. Edit a curve in Workspace
and an open Comparison window updates on its next repaint - there's no
separate "sync" step anywhere in the app.

## Core concepts

Xenovision standardizes on four curve types, plus two roles a curve can
play:

| Type | Meaning |
|---|---|
| **Sensitivity** | Unitless. A sample is meaningful only relative to the other curves in the same visual system - a receptor's response shape, not an absolute measurement. Always a Workspace curve, never a stimulus. |
| **Radiance** | Units of power per area (an explicit unit string you set). A measurement of light itself - emitted, reflected, or otherwise reaching a point. |
| **Reflectance** | Unitless, normalized so every sample is meant to fall in `[0, 1]` - a ratio describing a surface, not light itself. The Stimulus Editor flags (non-blocking) a curve with values outside that range. |
| **Absorption** | Unitless, every sample in `[0, 1]`: the fraction of light an absorber lets through at that wavelength (1 = nothing absorbed). Applied to a luminant by point-wise multiplication, with a weight exponent: `luminant × absorption^weight` (0 = no effect, 1 = as-is, 2 = twice as strong, like passing through it twice). Atmospheric notches are absorption curves. |

| Role | Meaning |
|---|---|
| **Luminant** | A Radiance curve used as the shared reference light every comparison is evaluated under. Managed in the Comparison window. |
| **Stimulus curve** | Either a Radiance curve directly, or a Reflectance curve - in which case Comparison automatically filters it through the current luminant (`Reflectance × Luminant`) before evaluating it, rather than treating the raw ratio as if it were already light reaching the eye. Lives in the shared stimulus library (Stimulus Editor window), never in a Workspace tab. |

Other terms used throughout:

| Term | Meaning |
|---|---|
| **Curve Set** | A named collection of receptor (Sensitivity) curves - one species' full visual system, open as a Workspace tab. |
| **Colorspace curve** | A receptor curve that feeds into a visual system's luminance/chroma math - its "cones." Filled dot (●) in the left rail. |
| **Isolated curve** | A receptor curve kept for reference but excluded from the colorspace math - e.g. non-color-vision photoreceptors, or (mantis shrimp) every receptor it has. Hollow dot (○). |
| **ω (Weber fraction)** | Per-receptor noise term. Blank means "no data," not zero. |
| **η (relative density)** | Per-receptor density, meaningful relative to the other receptors in the same set. Also blank-means-no-data. |
| **w_i (luminance weight)** | How much a receptor contributes to luminance. Blank = integral-derived default; set it to override. |
| **Receptor saturation** | The maximum signal a receptor can produce, however much power reaches it - its activation is capped at this value. Blank = no cap. Distinct from the perceptual *saturation* in Comparison's coordinate table (below). |
| **Saturation / hue angle** | For a species with N > 2 receptors, the chroma vector's length (saturation) and its N-2 hyperspherical angles (hue) - shown alongside luminance/chroma in Comparison's coordinate table. |
| **ΔS** | Vorobyev-Osorio perceptual distance (JND units) - needs ω and η on every receptor of the system being evaluated. |

## Getting started

On first launch, Xenovision bootstraps its built-in library - twelve species
fixtures - into your system's per-user data directory
(`xenovision/fixtures/*.json`, one file per species) and opens one Workspace
tab per species. The stimulus library starts seeded with three example
reflectance curves (Reddish/Greenish/Blueish) so Comparison has something to
work with immediately, and the luminant list starts with a flat "Uniform"
reference and a 5778K solar default. Nothing to configure.

## Workspace window

Three columns: left rail, center graph pane, right inspector. Drag the
dividers to resize.

**Menu bar** - File (save/load, restore defaults, export CSV), View (axis
orientation, global), Window (open, close, dock, or undock any panel - see
[Arranging windows](#arranging-windows)).

**Tabs** - one per open Curve Set. Type a name and click **+ New Curve Set**
to start an empty one. Click a tab's **×** to close it; if it has unsaved
changes, you're asked to confirm first (closing a tab really does discard
its data, unlike closing a whole window).

**Left rail** - a Colorspace/Isolated toggle picks which graph the center
pane shows, independent of selecting a curve. Below it, the two curve lists.
Click a name to select it; click it again to deselect. **+ New Curve** adds
a blank Sensitivity-typed curve to the colorspace list - Workspace only ever
creates receptor curves; stimulus curves and their generators/importers live
in the Stimulus Editor window instead (see below).

**The graph** - every curve in the active list is drawn, but only the
*selected* curve's points are draggable (this is deliberate, to stop a drag
near one curve from silently retargeting another). The x-axis shows numeric
wavelength labels in nm. Gestures:

| Gesture | Effect |
|---|---|
| Drag a point | Move it (selected curve only) |
| Arrow keys | Nudge the selected point (1 nm / ~1% of the value range per press; hold Shift for 10x) |
| Double-click empty space | Add a point to the selected curve |
| Right-click a point | Remove it |
| `Delete` / `Backspace` | Remove the selected point |
| Click a name in the legend row | Select it, or deselect if already selected |

Below the graph, while viewing **Colorspace**: a second "subjective
spectrum" bar, showing what this species' own receptor mix renders each
wavelength as (each colorspace curve's display color, blended per
wavelength by its response there, brightness-scaled by the total response) -
compare it against the physical-wavelength gradient bar above it to see how
a species' perception diverges from the raw spectrum. Beneath that, the
opponent-contrast table (signed weight per receptor; leave empty for the
automatic per-receptor fallback) and a remove-curve button. Isolated view
shows an add-isolated-curve button instead, since opponent contrasts don't
apply there. Undo/redo at the bottom covers every editing surface on the tab
as one shared history (`Ctrl+Z` / `Ctrl+Shift+Z` or `Ctrl+Y`).

With more than 32 colorspace curves, an amber note warns that each edit to
the set pauses briefly (around a second or more) while its adaptation matrix
is re-derived. Smaller systems take milliseconds.

**Right inspector** - curve name; a numeric wavelength/value editor for
whichever point is currently selected on the graph (not a table of every
point - that overwhelmed the panel and was removed); noise/luminance fields
(colorspace curves only: ω, η, w_i); receptor saturation (every curve,
colorspace or isolated); free-form metadata; and an amber
validation flag if η is set on *some but not all* colorspace curves in the
set.

**File menu**

| Action | What it does |
|---|---|
| Save | Writes the active tab to its current file path, or a name-derived default if never saved. |
| Save As... | Writes to the typed path and remembers it for future Saves. |
| Restore Default Fixtures... | Resets any open tab matching a built-in species' name back to shipped data. Custom tabs are untouched. |
| Export selected curve as CSV | Writes the selected curve's points as `wavelength_nm,value` rows. |

## Comparison window

Top bar, then three columns.

**Luminant** - the dropdown picks the shared reference light every result in
this window uses. **+ New luminant ▾** adds one (blank/black-body/composite
LED, each tagged Radiance automatically) and makes it active; with one
selected, a name field and the same drag-to-edit graph as Workspace appear
below it. **Remove this luminant** deletes the active one.

Under the luminant's graph, its total integrated power is shown with two
ways to change it without altering its spectral shape: **Scale by** (enter
a factor, then Apply) or **Set total power to** (enter a target, then Set -
disabled for a zero-power curve). This matters once receptors have a
saturation cap: the same spectrum at higher power can push receptors into
their caps and change the results.

**Apply absorption** (below that) multiplies the luminant by an absorption
curve from the Stimulus Editor's library, raised to the chosen **weight**.
This is how notches are added to a luminant: create the absorption curve
first (from a notch preset or a custom notch), then apply it here. Each
application is recorded in the luminant's metadata. Absorption curves
don't appear in the input-spectra checklist, since they aren't stimuli.

**Input spectra (left column)** - a checklist against the shared stimulus
library (managed in the Stimulus Editor window) - tick a curve to include it
in this comparison. If the library is empty, create a curve there first. A
Reflectance-typed stimulus is automatically filtered through the current
luminant before evaluation (`Reflectance × Luminant`); a Radiance-typed one
is used as-is.

**Species (right column)** - every open Workspace tab as a checkbox, with a
provenance dot: green = complete per-receptor noise data (ΔS computable),
amber = incomplete (ΔS unavailable for that species).

**Results (center column)** - adapts to selection:

- **One species checked** - its coordinate table (luminance + chroma per
  input spectrum, plus, for N > 2 receptors, Saturation and Hue φ1..φ(N-2)
  columns), and, with 2+ inputs, a difference matrix under whichever metric
  is selected in the top bar (Euclidean, chroma-only, or ΔS).
- **Two or more species** - the same tables behind per-species sub-tabs, plus
  a cross-species ΔS summary table beneath: every stimulus pair as a row,
  every species as a column, always under ΔS regardless of the top bar's
  metric choice (it's the only metric meaningful across species with
  different receptor counts).

Results recompute live from current selections - nothing here needs a manual
refresh.

## Stimulus Editor window

Where every stimulus curve (reflectance or radiance) is created and
edited - the counterpart to Workspace, but for the shared stimulus
library instead of visual systems. Left rail: the library, with a **+ New
stimulus ▾** menu and a **×** per curve to unload it (warns first if it has
unsaved changes, same as closing a Workspace tab).

**+ New stimulus ▾** offers:

| Option | What it does |
|---|---|
| Blank curve | An empty Reflectance-typed curve. |
| Edit a copy of... | Duplicates an existing library curve under a new name. |
| Derive reflectance from radiance... | Divides a chosen Radiance curve by a chosen luminant (`Reflectance = Measured ÷ Luminant`). Both need matching unit text, set via the source curve's own editor first. |
| Derive radiance from reflectance... | Multiplies a chosen Reflectance curve by a chosen luminant, predicting its appearance under that light. |
| Import from image... | Opens an embedded image-loading/region-selection flow (below); extracting a region adds it straight to the library. |
| Import from text/CSV... | Parses pasted wavelength,value rows (comma- or whitespace-separated; header/non-numeric lines are skipped). Wavelengths under 50 are assumed to be micrometers (the USGS spectral library convention) and converted to nm automatically. |
| Blank absorption curve | A flat absorption curve at 1.0 (absorbs nothing) to shape by hand. |
| Absorption curve from notch preset | One Gaussian notch from the preset library (e.g. atmospheric water-vapor and oxygen bands) as an absorption curve. |
| Custom notch... | Set a notch's name, center, width, and depth, then **Create** it as an absorption curve. **Save as notch preset** adds it to the preset library for reuse. |

Selecting a curve in the rail shows its editor: name, a quantity-kind/unit
picker (Unspecified/Reflectance/Transmittance/Radiance/Absorption -
Sensitivity is deliberately not offered here, since that's reserved for
visual systems), an amber warning if a Reflectance or Absorption curve has a
value outside `[0, 1]`, the
same drag-to-edit graph and single-selected-point numeric fields as
Workspace, and Save As.

**Import from image**: load an ENVI (point
at the `.hdr`) or GeoTIFF file, or list and pick variables from a MATLAB
`.mat` file; assign a wavelength axis if the file has none (a matching
sensor preset, or a manual comma-separated list); preview any band; select a
**Rectangle** or **Polygon** region on the preview; tick **Extract as an
Illumination/radiance curve** if the region is a light source rather than a
reflectance sample; then **Extract region to curve**, which adds the
result straight to the library (you can keep extracting more regions from
the same image, or click **Done** to return to normal editing).

A region extracted as a reflectance sample is tagged Reflectance. One
extracted as a light source is tagged Radiance with unit "relative", since
image values are in sensor units rather than calibrated ones. That matches
the built-in luminants' unit, so it works with **Derive reflectance from
radiance...** directly; change the unit in the curve's editor if you know
the image's calibration.

## How the windows connect

All three windows read and write one shared pool of data - there's no
per-window copy of anything.

- Edit a curve's points, noise data, or opponent contrasts in Workspace, and
  an open Comparison window reflects it on its very next repaint. The same
  goes for editing a stimulus curve in the Stimulus Editor.
- Closing the Comparison or Stimulus Editor *panel* doesn't discard what's
  in it - selections, a loaded image, scroll position, and so on persist
  until you reopen it. Closing a Workspace *tab* (a visual system) or
  unloading a stimulus curve is different: that data really is discarded
  (hence the unsaved-changes warning first).

## Arranging windows

The main window shows a strip of panel tabs along its top. Each panel is
in one of three places:

- **Docked** - a tab in the main window. Click the tab to switch to it.
- **Floating** - in its own separate window, which you can move to another
  monitor.
- **Closed** - not shown (Comparison and Stimulus Editor only; their
  contents are kept for when you reopen them).

| To... | Do this |
|---|---|
| Pop a docked tab out into its own window | Click **⇱** on its tab. |
| Merge a floating window back in as a tab | Click **⇲ Dock into main window** at the top of that window. |
| Close a panel | Click **×** on its tab, or close its floating window. |
| Open a closed panel | **Windows ▾** at the end of the tab strip (or Workspace's **Window** menu), then **Open as tab** or **Open as window**. |

Every panel stays usable in a small window: below its minimum size
(about 960×640 for Workspace, 900×520 for Comparison, 800×520 for
Stimulus Editor) it keeps its layout and gains scrollbars instead of
squashing. Comparison also scrolls vertically on its own, and its results
column scrolls sideways when tables are wide.

Two rules keep the layout usable: the main window always keeps at least one
tab (so ⇱ and × are disabled on the last one), and Workspace can't be
closed - closing its floating window docks it back into the main window
instead.

## Keyboard & mouse reference

| Input | Where | Effect |
|---|---|---|
| `Ctrl+Z` | Workspace, active tab | Undo the last edit on this tab |
| `Ctrl+Shift+Z` or `Ctrl+Y` | Workspace, active tab | Redo |
| Arrow keys | Any curve graph, point selected | Nudge it (Shift = 10x step) |
| `Delete` / `Backspace` | Any curve graph, point selected | Remove that point |
| Drag | Any curve graph | Move the selected curve's point under the cursor |
| Double-click | Any curve graph, empty area | Add a point to the selected curve |
| Right-click | Any curve graph, on a point | Remove that point |

"Any curve graph" covers the Workspace center pane, Comparison's luminant
editor, and the Stimulus Editor's curve editor - they're all the same
widget. Arrow-key and Delete/Backspace nudging only fire while no text field
or other widget has keyboard focus, so they won't fight with typing in a
name or metadata field.

## Troubleshooting

**Startup log warnings about EGL, MESA, or "ZINK: failed to choose pdev"** -
the graphics driver falling back to software rendering on systems without a
working GPU path (common under WSL). Harmless; the app runs normally.

**Nothing happens when I click a point on the graph.** - Points are only
draggable on whichever curve is currently selected. Click its name in the
left rail or the legend row under the graph first.

**A curve I expected to see in Comparison's input-spectra checklist isn't
there.** - That checklist only shows curves in the shared stimulus library.
Create or import it in the Stimulus Editor window first.

**I can't find where to turn a receptor curve into a stimulus, or vice
versa.** - You can't, by design: a visual system's receptor curves
(Workspace) and the stimulus library (Stimulus Editor) are deliberately
separate pools. If you want a stimulus with a particular shape, build it in
the Stimulus Editor (e.g. "Edit a copy of..." an existing one, or a blank
curve) rather than repurposing a receptor curve.

**ΔS says "unavailable" for a species.** - It needs ω and η set on every one
of that species' colorspace curves. Check the inspector's noise fields, or
look for the amber partial-coverage warning.

## Built-in species reference

Twelve fixtures load as Workspace tabs on first run:

- Human (*Homo sapiens*)
- Dog (*Canis lupus familiaris*)
- Cat (*Felis catus*)
- Mouse (*Mus musculus*)
- Goldfish (*Carassius auratus*)
- Frog (*Rana* spp.) - Scotopic
- Frog (*Rana* spp.) - Photopic
- Pigeon (*Columba livia*)
- Honeybee (*Apis mellifera*)
- Swallowtail Butterfly (*Papilio xuthus*)
- Mantis Shrimp (Stomatopoda)
- Penguin (*Spheniscus humboldti*)

Each is independently editable and independently restorable to its shipped
defaults from Workspace's File menu. Mantis Shrimp is the one notable
structural outlier: all twelve of its receptor classes live in **Isolated**,
by design - the fixture models the hypothesis that they aren't combined into
a human-style opponent colorspace at all, so its luminance and chroma are
always exactly zero.
