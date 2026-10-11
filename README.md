# Xenovision User Guide

A reference to every window, menu, and gesture in the current build.

> An illustrated version of this guide - with a screenshot of every window - is
> in [`docs/user-guide.md`](docs/user-guide.md). That is the document shipped as
> `user-guide.pdf` in the release packages.

## Overview

Xenovision models and compares how different species perceive color. You
define a **visual system** as a set of photoreceptor sensitivity curves,
Xenovision derives that system's luminance and chroma channels from them, and
then evaluates arbitrary **stimulus spectra** (reflectance samples, light
sources) through it to get perceptual coordinates and distances. Eleven
literature-based species are built in, from trichromatic humans to the
twelve-receptor mantis shrimp, and you can define your own alongside them.

The interface is four panels - **Workspace**, **Comparison**, **Stimulus
Editor**, and **Image Display** - sharing one window model (see
[Arranging windows](#arranging-windows)): each panel is a **container** with
its own top-level tab in the main window, each container holds one or more
**instances** as tabs beneath it, and any tab - container or instance - can be
popped out into its own OS window and docked back.

- **Workspace** - define and edit visual systems: receptor curves, noise
  data, opponent contrasts. Always open; each of its instances is a Curve Set.
- **Comparison** - run stimuli through one or more visual systems and read
  off coordinates, distances, and cross-species summaries.
- **Stimulus Editor** - create and edit the stimulus curves (reflectance or
  radiance) that Comparison evaluates - by hand, derived from another curve,
  or imported from an image or a text/CSV file.
- **Image Display** - render a loaded image (or one region of it) through a
  visual system's receptors and inspect the result channel by channel.

Comparison, Stimulus Editor, and Image Display are **repeatable**: open
several independent instances at once, each with its own setup, from a
container's **＋ New** button or the **Windows** menu. They start closed on
first launch.

All four read and write the same underlying data. Edit a curve in Workspace
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
| **Regime** | A named colorspace: a set of pool receptors that jointly produce luminance + chroma, with its own membership, luminance weights, and opponent contrasts. Most species have one (their whole eye); the frog has three (scotopic/mesopic/photopic, §2.3.10); a receptor can be in several. |
| **Unaffiliated receptor** | A receptor in the pool that belongs to no regime - e.g. non-color-vision photoreceptors (swallowtail butterfly's violet/broad-band classes), or (mantis shrimp) every receptor it has. Draws a hollow dot (○) in the left rail, reported individually, never part of any luminance/chroma calculation. |
| **ω (Weber fraction)** | Per-receptor noise term. Blank means "no data," not zero. |
| **η (relative density)** | Per-receptor density, meaningful relative to the other receptors in the same set. Also blank-means-no-data. |
| **w_i (luminance weight)** | How much a receptor contributes to luminance. Blank = integral-derived default; set it to override. |
| **Receptor saturation** | The maximum signal a receptor can produce, however much power reaches it - its activation is capped at this value. Blank = no cap. Distinct from the perceptual *saturation* in Comparison's coordinate table (below). |
| **Operating window** | The range of ambient irradiance (W/m²) a receptor operates in, as seen by that receptor (the light its own sensitivity absorbs from the reference). Below the window's minimum or above its maximum the receptor gates off; between them its gain is ~1, with soft logistic shoulders (blank = use the Curve Set's defaults; blank everywhere = always on). This is how the same eye produces different color perceptions at dim vs. bright light levels - e.g. the frog's scotopic/mesopic/photopic regimes. |
| **Saturation / hue angle** | For a species with N > 2 receptors, the chroma vector's length (saturation) and its N-2 hyperspherical angles (hue) - shown alongside luminance/chroma in Comparison's coordinate table. |
| **ΔS** | Vorobyev-Osorio perceptual distance (JND units) - needs ω and η on every receptor of the system being evaluated. |

## Getting started

On first launch, Xenovision bootstraps its built-in library - eleven species
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

**Left rail** - "Graph shows" pills pick what the center graph displays:
"All pooled" (every receptor) or one named regime's members. Below them, the
single receptor-pool list. A filled dot (●) marks a receptor that belongs to at
least one regime; a hollow dot (○) marks an unaffiliated one (in no regime),
with an "Unaffiliated" group repeated below for quick access. Click a name to
select it; click it again to deselect. **+ New Curve** adds a blank
Sensitivity-typed receptor to the pool (joining the regime you're currently
viewing, or starting unaffiliated if the graph shows "All pooled") - Workspace
only ever creates receptor curves; stimulus curves and their
generators/importers live in the Stimulus Editor window instead (see below).

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

Below the graph, a "subjective spectrum" bar spans the whole receptor
pool: what this species' own receptor mix renders each wavelength as (each
receptor's display color, blended per wavelength by its response there,
brightness-scaled by the total response) regardless of the graph filter -
compare it against the physical-wavelength gradient bar above it to see how
a species' perception diverges from the raw spectrum.

Beneath the graph, the **Colorspace regimes** panel: add/rename/remove
regimes, a membership table (which pool receptors belong to which regime),
each regime's derived light-level band (the intersection of its members'
operating windows), per-regime luminance-weight overrides and opponent-contrast
definitions (signed weight per member; leave empty for the automatic
per-receptor fallback), and the natural-scene auto-generator for one regime's
contrasts. Structural warnings live here too: an empty regime, unaffiliated
receptors, or disjoint member windows (a regime that can never turn on).
A **Remove this curve** button under the graph deletes the selected receptor
from the pool and every regime at once. Undo/redo at the bottom covers every
editing surface on the tab as one shared history (`Ctrl+Z` / `Ctrl+Shift+Z` or
`Ctrl+Y`).

With more than 32 receptors visible in the graph, an amber note warns that
each edit to the set pauses briefly (around a second or more) while its
adaptation matrices are re-derived. Smaller systems take milliseconds.

**Right inspector** - curve name; a numeric wavelength/value editor for
whichever point is currently selected on the graph (not a table of every
point - that overwhelmed the panel and was removed); the receptor's regime
membership (or "unaffiliated"); noise/luminance fields (ω, η, w_i, for any
receptor); receptor saturation; the receptor's operating window (W/m²
seen-irradiance min/max + shoulder softness, with the effective window -
including the set's defaults - shown beneath); the set's operating-window
defaults (always shown, top of the inspector; applies to any receptor
without its own window); free-form metadata; and amber validation flags if
η is set on *some but not all* regime members in the set, if any operating
window is malformed (min ≥ max, or a non-positive bound), if a regime is
empty, if receptors are unaffiliated, or if a regime's member windows are
disjoint (it can never be active at any light level).

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

Under the luminant's graph, its total irradiance is shown (W/m², integrated
over its domain — under Xenovision's convention luminant curves are spectral
irradiance in W/m²/nm) with two ways to change it without altering its spectral
shape: **Scale by** (enter a factor, then Apply) or **Set total power to** (enter
a target, then Set - disabled for a zero-power curve). This matters once receptors
have a saturation cap **or an operating window** (below): the same spectrum at
higher power can push receptors into their caps, gate whole receptor classes on
or off across an operating window, and so change the results.

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

## Image Display window

Renders an image through a visual system's receptor channels: each output
channel is one receptor's activation, integrated over every pixel's actual
spectrum. A left rail, then a scrolling list of **views**.

**Left rail** - load an ENVI (point at the `.hdr`), GeoTIFF, or MATLAB
`.mat` file, or pick from the list of loaded images. The image list is shared
with the Stimulus Editor, so an image loaded in either place is available in
both (**✕** unloads it). **+ Add view for selected image** and **+ Add blank
view** start a view; **＋ New Image Display window** opens an independent
second panel. Every panel shares the loaded images but keeps its own views.

**Each view** is one configured render. Its **Settings** header (collapsed by
default) picks the **Image**; the **Visual system** and **Regime** (or the
system's dominant one); the **Stage** (**raw receptor**, **von Kries
adapted**, or **perceptual**; a species with no regime is raw-only, one panel
per receptor); how to treat the pixels (**Reflectance** or **Radiance**) and,
for reflectance, the **Luminant**; the **Region** (whole image, a region saved
in the Stimulus Editor, or one drawn here); the integration **step**; and the
display mapping. **Duplicate** copies a view and **✕** removes it.

**Display** is either **one panel per channel** (each with a selectable
colormap and a low→high swatch) or a single **composite image**: the
perceptual stage gets an `L*`-style `[L, C_a, C_b]` encoder (both chroma axes
selectable, so a regime with more than three receptors can show any pair),
and any stage can instead map three chosen channels straight to R/G/B.

Two things are always on screen so a result is never ambiguous: a
**provenance bar** naming every input (source and band, region, input kind +
luminant, species + regime, stage, step, GPU/CPU) and a **false-color
legend** spelling out the mapping actually used. Hovering an image reads out
that pixel's channel values.

Rendering runs off the UI thread, so a large cube never freezes the window;
while a render is in flight the previous image stays on screen with a small
"rendering…" note (a spinner if there's nothing to show yet).

From the Stimulus Editor's image import pane, **Show channels in Image
Display** opens (or reuses) an Image Display window already seeded with the
image and region you were looking at.

## How the windows connect

All four windows read and write one shared pool of data - there's no
per-window copy of anything.

- Edit a curve's points, noise data, or opponent contrasts in Workspace, and
  an open Comparison window reflects it on its very next repaint. The same
  goes for editing a stimulus curve in the Stimulus Editor.
- The image library and its named regions are shared too: load an image once,
  in the Stimulus Editor or any Image Display window, and every Image Display
  view can render it.
- Closing a repeatable instance (Comparison, Stimulus Editor, Image Display)
  doesn't discard its setup - the last one closed is *parked*, and the next
  instance of that kind inherits it when no other instance of that kind is
  open. Closing a Workspace *tab* (a visual system) or unloading a stimulus
  curve is different: that data really is discarded (hence the
  unsaved-changes warning first).

## Arranging windows

The main window shows a strip of **container** tabs along its top - one for
each panel group currently docked in it. Selecting a container shows its
**instance** tabs beneath it and the active instance's contents. A container
and an instance are each independently **docked** (a tab in the main window)
or **floating** (its own separate window, which you can move to another
monitor):

- A **container** is one whole panel group: the Workspace with all its Curve
  Sets, or all the Comparison instances, and so on.
- An **instance** is one tab inside a container: a Workspace Curve Set, or one
  Comparison / Stimulus Editor / Image Display panel.

A kind's container tab is shown only while it has at least one instance still
inside it. On first launch, Comparison, Stimulus Editor, and Image Display
have no instance and so no tab.

| To... | Do this |
|---|---|
| Pop a docked tab out into its own window | Click **⇱** on the tab (a container tab floats the whole group; an instance tab pops out just that one), or drag the tab away from its strip and release. |
| Merge a floating window back in as a tab | Click **⇲ Dock into main window** at the top of that window, or drag its title tab back over the main window and release. |
| Make a new instance | The **＋ New** button on a container's own tab bar, or **Windows ▾**. |
| Close an instance | Click **×** on its tab, or close its floating window. |
| Move or open any panel | **Windows ▾** at the end of the tab strip (or Workspace's **Window** menu): every container and instance is listed with a Float/Dock and a Close action. |

While you drag a tab, a ghost chip follows the pointer, even into another OS
window. Release it over the main window and the chip turns green with the main
window outlined, showing that the drop will dock rather than float; releasing
it back in its own strip just selects it instead.

Every panel stays usable in a small window: below its minimum size (about
960×640 for Workspace, 900×520 for Comparison, 800×520 for the Stimulus
Editor, 900×560 for Image Display) it keeps its layout and gains scrollbars
instead of squashing. Comparison also scrolls vertically on its own, and its
results column scrolls sideways when tables are wide.

You can float every container, leaving the main window empty (it then reads
"No windows open"); closing a floating *container* window docks that group
back into the main window rather than closing it, so no group is ever lost;
closing a floating *instance* window closes just that one instance.

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
| Drag a tab | Any tab bar | Pull it past a short distance and release to pop it out; release it back in the bar to just select it |
| Drag a floating window's title tab | Top of a floating window | Drop it over the main window to re-dock that window |

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
of that species' colorspace regime members. Check the inspector's noise fields,
or look for the amber partial-coverage warning.

## Built-in species reference

Eleven fixtures load as Workspace tabs on first run:

- Human (*Homo sapiens*)
- Dog (*Canis lupus familiaris*)
- Cat (*Felis catus*)
- Mouse (*Mus musculus*)
- Goldfish (*Carassius auratus*)
- Frog (*Rana* spp.) - one tab carrying all three regimes: Scotopic
  (rods-only), Mesopic (all five receptors), Photopic (cones-only)
- Pigeon (*Columba livia*)
- Honeybee (*Apis mellifera*)
- Swallowtail Butterfly (*Papilio xuthus*)
- Mantis Shrimp (Stomatopoda)
- Penguin (*Spheniscus humboldti*)

Each is independently editable and independently restorable to its shipped
defaults from Workspace's File menu. Mantis Shrimp is the one notable
structural outlier: it declares **no colorspace regime at all**, by design -
the fixture models the hypothesis that its 12 receptor classes are never
combined into a human-style opponent colorspace, so every receptor reports
individually and its luminance and chroma are always exactly zero. Frog is
the other structural outlier, in the opposite direction: three regimes over
one shared receptor pool, so that at any light level one of them (the
dominant, per-receptor-gain-max) drives the comparison.
