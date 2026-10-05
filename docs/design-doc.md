# Cross-Platform Color Perception Modelling App — Design Specification

## 1. Spectral Curve Sets: Storage, Display, and Editing

### 1.1 Purpose
Provide a data model, visual representation, and editing workflow for spectral curves (reflectance, radiance/emission, transmittance, photoreceptor sensitivity, or any other wavelength-indexed quantity), organized into named sets representing a coherent visual system or measured dataset.

### 1.2 Data Model

#### 1.2.1 Spectral Curve
A spectral curve is a general-purpose, typed container for arbitrary (wavelength, value) point pairs — not a fixed-resolution sampled array.

- **Points**: arbitrary list of `(wavelength_nm: f64, value: f64)` pairs. No fixed range or resolution; points may be irregularly spaced.
- **Curve type (metadata tag)**: an enum/string indicating what the curve represents — e.g. `Reflectance`, `Emission`, `Transmittance`, `Sensitivity`, `Illumination`, `Other(String)`.
- **Quantity kind and unit**: a fixed enum of physically meaningful quantity kinds, each with an associated expected unit (or explicitly "unitless"), extending the curve type tag rather than existing as a separate parallel field:
  - `Reflectance` — dimensionless, expected range 0–1.
  - `Transmittance` — dimensionless, expected range 0–1.
  - `Sensitivity` — dimensionless/relative (typically normalized to peak = 1).
  - `Radiance` — physical radiometric unit (e.g. W·m⁻²·sr⁻¹·nm⁻¹).
  - `Irradiance` — physical radiometric unit (e.g. W·m⁻²·nm⁻¹).
  - `Unspecified` — no quantity kind/unit declared (the default).
  - Quantity kind/unit is optional at curve creation for general use (display, editing); it is required only for operations that depend on physical unit consistency (§5.3.1).
- **Interpolation**: monotonic cubic interpolation (e.g. Fritsch–Carlson or PCHIP-style) between points, used for both display and any calculation requiring a value at an arbitrary wavelength. Monotonic variants avoid overshoot artifacts (e.g. negative reflectance values) that plain cubic splines can introduce.
- **Name/label**: human-readable identifier for the curve (e.g. "L-cone sensitivity", "Petal reflectance").
- **Optional metadata**: free-form key-value notes (e.g. source citation, measurement conditions, data-quality/provenance annotations).

#### 1.2.2 Curve Set
A named collection of curves representing a coherent group — typically one species' full visual system (all photoreceptor sensitivity curves) or one measured object's properties.

- **Name**: human-readable identifier (e.g. "Honeybee (Apis mellifera)").
- **Colorspace curves**: ordered list of Spectral Curve objects (§1.2.1) that feed into this set's opponent-process colorspace (luminance + chroma, §2.2) - the set the perceptual pipeline operates on.
- **Isolated curves**: a second, separate list of Spectral Curve objects kept with the set for completeness/inspection but excluded from the colorspace entirely - e.g. a species' photoreceptor classes that are documented but don't participate in color-opponent perception (§2.3.9's swallowtail butterfly), or - the extreme case - a species whose entire receptor set is isolated and whose colorspace list is empty (§10.2's Mantis Shrimp). Individual activations for isolated curves are still computable and displayable (§10.1) even though they never contribute to luminance or chroma.
- **Optional metadata**: free-form notes at the set level (e.g. citation for the whole dataset).
- **Opponent contrast definitions** (§4.2.4): optional set-level field storing literature-known opponent pairings as explicit structured data.

No deeper hierarchy (e.g. sub-groups within a set) is included.

### 1.3 File Format / Persistence
Curve Sets are persisted as a JSON-based format (via `serde`) mirroring the data model in §1.2 directly, with an optional CSV export/import path for interoperability with existing spectral datasets.

### 1.4 Graphical Display

#### 1.4.1 Layout
- **Single plot, overlaid curves**: all curves in the active set (or active selection) are drawn on one shared graph, X = wavelength, Y = curve value.
- **Background spectrum band**: a fixed gradient strip rendered along the X-axis as a visual spectral reference:
  - Visible range (~380–700nm): standard rainbow spectrum.
  - Infrared (above ~700nm): fades from dark red toward black.
  - Ultraviolet (below ~380nm): fades from light blue toward white.
  - This gradient is fixed and not user-configurable.

#### 1.4.2 Curve Coloring
Each curve is drawn in a single representative color computed from its own data:

- The curve's values are treated as weights over wavelength and convolved against CIE color matching functions (x̄, ȳ, z̄) to obtain XYZ, then converted to a display RGB via sRGB conversion with gamut clipping/normalization.
- **Normalization**: each curve's own values are normalized (e.g. peak or integral normalized to 1) before integration, so curve shape — not absolute magnitude — determines color.
- **Out-of-CIE-gamut wavelengths**: for curve weight lying outside the ~360–830nm range CIE color matching functions are defined for, the background-gradient's UV/IR color logic (§1.4.1) is used as a fallback, for visual consistency between the two coloring systems.

### 1.5 Editing
Ergonomic editing combines direct graph manipulation with a synced numeric view:

- **Graph interaction**: points are rendered as draggable handles directly on the curve. Dragging updates the point's (wavelength, value) live, with the curve redrawing in real time. Supports adding a point (click/double-click on empty curve area) and deleting a point (right-click or delete key on a selected handle).
- **Numeric table**: a synced, editable table (columns: wavelength, value) alongside the graph, operating on the same underlying point list, staying in sync bidirectionally with the graph.
- **Multi-curve editing context**: when multiple curves are displayed, the table view indicates which curve is active (e.g. tabs, or color-coded rows matching curve colors from §1.4.2).

---

### 1.6 Multi-System Workspace Layout

When more than one Curve Set is open or available for editing (e.g. two or more species loaded simultaneously, or a single species with multiple regime-specific systems such as frog's scotopic/photopic pair, §2.3.10), the workspace uses a **tabbed layout**: one tab per Curve Set. Selecting a tab switches the entire per-set workspace — the curve display/editor (§1.4–1.5), the noise/luminance-weight table (§4.3), and the opponent contrast definition view (§4.3) — to that Curve Set's data as a single, consistent unit, rather than allowing different panels to independently point at different sets.

This tabbed model applies specifically to the **single-set editing and inspection workspace**. It does not apply to views that are inherently cross-set by design — the multi-spectrum coordinate table and difference matrix (§3.3), the cross-species summary view (§6.3.2), and any similar comparison view continue to operate as separate, non-tabbed views that draw on data from multiple Curve Sets (potentially multiple tabs' worth) at once, since collapsing those into a single-tab-at-a-time model would defeat their purpose.

### 1.7 Undo/Redo

Each open Curve Set (i.e. each tab, §1.6) maintains its own **linear undo/redo history**, covering every editing action within that Curve Set's workspace uniformly: curve point additions/moves/deletions (§1.5), noise/luminance-weight table edits (§4.3), and opponent contrast definition edits (§4.3) are all recorded onto the same per-tab history stack, in the order performed, rather than each editing surface maintaining a separate independent history.

This follows directly from §1.6's framing of a tab as a single consistent workspace unit for one Curve Set — undo/redo uses the same scoping boundary, so a single "undo" always steps back one action within the currently active tab's complete edit history, regardless of which panel (graph, table, opponent view) that action was performed in, rather than requiring the user to know which specific sub-history to undo from.

Switching tabs (§1.6) does not affect either tab's independent history; returning to a previously-active tab preserves its undo/redo stack exactly as it was left. History is not persisted across saves or application restarts — reopening a saved Curve Set (§1.3) begins with an empty history, consistent with undo/redo being an in-session editing aid rather than a durable record (durable snapshots of prior states, where desired, are instead served by Save As, §9, which captures an explicit point-in-time copy rather than a step-by-step history).

---

## 2. Photoreceptor-to-Perceptual-Channel Transform

### 2.1 Purpose
Given a reference spectral curve representing average environmental light exposure, compute a matrix transform converting raw photoreceptor activations into:
- 1 luminance channel (achromatic brightness signal)
- (N-1) opponent chroma channels (N = number of receptor types)
- 1 derived saturation scalar (magnitude of the chroma vector)

This generalizes trichromatic human color-opponent theory to arbitrary N-receptor visual systems.

### 2.2 Mathematical Model

#### 2.2.1 Pipeline overview
```
Reference illuminant spectrum ──┐
                                  ├─► Chromatic adaptation (CAT) ─► Adapted receptor activations
Raw receptor sensitivity curves ┘                                          │
                                                                             ▼
                                                              Luminance + Opponent transform matrix
                                                                             │
                                                                             ▼
                                                        [Luminance, Chroma_1, ..., Chroma_(N-1)]
                                                                             │
                                                                             ▼
                                                              Saturation = ‖Chroma vector‖
```

#### 2.2.2 Step 1 — Raw receptor activation
For each receptor type `i` with sensitivity curve `S_i(λ)`, and an incident spectrum `I(λ)`:
```
Q_i = ∫ S_i(λ) · I(λ) dλ
```
Computed numerically via the monotonic cubic interpolation (§1.2.1), over the union of wavelength support for `S_i` and `I`.

#### 2.2.3 Step 2 — Chromatic adaptation
The reference environmental spectrum `E(λ)` is the adapting white point; its per-receptor activation `Q_i^E` is computed per §2.2.2. Chromatic adaptation proceeds as:

1. Transform raw receptor activations into a species-specific "adaptation space" via an N×N matrix `M_adapt`.
2. Scale each adaptation-space channel by the inverse of that channel's response to the reference illuminant (von-Kries-style diagonal scaling).
3. Transform back via `M_adapt⁻¹` to get adapted receptor activations `Q_i'`.

**`M_adapt` derivation**: computed generically for every species (including humans) by **minimizing pairwise spectral overlap** between the transformed receptor basis — finding the N×N linear transform of the species' receptor sensitivity curves that produces the most mutually exclusive (least-overlapping) effective channels, evaluated as the sum of pairwise overlap integrals across wavelength. This is self-contained, requiring only the species' own receptor curves (no external natural-spectra corpus dependency). The optimization constrains solutions to physically reasonable linear combinations, avoiding degenerate or sign-flipped channels.

This approach is used (rather than statistical decorrelation against a natural-spectra corpus, or co-deriving adaptation jointly with the opponent transform) because: (a) a natural-spectra corpus choice would make `M_adapt` an artifact of dataset selection rather than a property of the visual system, and (b) keeping adaptation and opponency as independent pipeline stages allows each to be validated separately. For the human receptor set, this derivation produces a matrix reasonably close in structure to literature CAT02/Bradford matrices, since those are themselves approximately narrow-band-optimized transforms of human LMS — serving as a validation check on the method.

#### 2.2.4 Step 3 — Luminance + Opponent transform
Given adapted activations `Q' = [Q'_1, ..., Q'_N]`:

**Luminance channel:**
```
L = Σ (w_i · Q'_i)   for i = 1..N
```
where `w_i` are species-specific luminance weights (§4.2.3) reflecting each receptor's known contribution to achromatic/motion vision. Every receptor contributes to luminance; none is excluded.

**Chroma channels (generalized Hering opponency via orthogonalized candidate contrasts):**

Every receptor must contribute to *both* luminance and chroma — no receptor is excluded from either. Construction proceeds in two stages:

1. **Candidate contrasts.** For each receptor `i`, construct a candidate opponent contrast against the weighted mean of all other receptors:
   ```
   Cand_i = Q'_i − Σ_{j≠i} (v_ij · Q'_j)
   ```
   Where species-specific literature identifies established opponent mechanisms (e.g. human L−M and S−(L+M); honeybee RNL-model mechanisms), those literature-defined contrasts are used in place of the generic formula, stored as explicit opponent contrast definitions (§4.2.4). This produces N candidate vectors, one per receptor.

2. **Orthogonalization against luminance.** The N candidate contrasts are Gram-Schmidt-orthogonalized against the luminance vector `L` and against each other, removing any luminance-direction component from each candidate (so chroma is independent of brightness) and reducing the N candidates to N-1 independent chroma axes.

This two-stage construction is kept as an explicit step after adaptation/sharpening — rather than treating the sharpened basis from §2.2.3 itself as the chroma output — because sharpening optimizes for minimal spectral overlap (a mathematical decorrelation property), which does not guarantee the resulting channels correspond to perceptual opponency (a physiological arrangement with directional valence, e.g. red-vs-green, not merely two statistically independent channels).

**Resulting matrix form**: the combined operation (§2.2.3–2.2.4) collapses into a single N×N matrix `T` (per species, per reference illuminant) such that:
```
[L, C_1, ..., C_(N-1)]ᵀ = T · Q
```
This matrix is precomputed once per species + reference illuminant pair, then reusable for fast evaluation of arbitrary stimuli.

#### 2.2.5 Step 4 — Saturation
```
Saturation = sqrt(Σ C_i²)   for i = 1..(N-1)
```
Retained alongside (not instead of) the full chroma vector.

### 2.3 Default Receptor Curves and Species Models

Peak sensitivities (λmax) below are literature-sourced (see References). Ten species/systems are provided as default/example models (§7.1, §8) — nine distinct species plus frog's second, regime-specific system — spanning dichromats, trichromats, and tetrachromats across mammals, birds, fish, insects, and amphibians, with two species illustrating structural variations on the standard single-regime, single-Curve-Set model: swallowtail butterfly (§2.3.9) has a photoreceptor complement exceeding the receptor count used for color-opponent perception, and frog (§2.3.10) has two entirely separate, regime-specific visual systems (scotopic and photopic) rather than one. Curve shapes are generated using standard photopigment absorption templates (e.g. Govardovskii et al. 2000) parameterized by each λmax, rather than hand-digitized, unless higher-fidelity digitized curves are substituted. Template family (A1/A2 chromophore) is a generation-time detail, not a stored data field.

**Note on mesopic vision**: several species in this document (notably frog, §2.3.10) have documented mesopic vision, in which rod and cone signals combine and jointly contribute to color perception at intermediate light levels, rather than operating as two cleanly separable regimes. This document does not model mesopic combination — each regime is modeled as an independent Curve Set with its own independent application of the full pipeline (§2.2), and no mechanism is specified for combining rod-regime and cone-regime receptor activations into a single joint perceptual calculation. This is a scoping boundary rather than an oversight: modeling mesopic combination would require specifying how rod and cone contributions are weighted relative to each other as a function of absolute light level, which is a substantially different kind of input (an illumination *intensity* parameter, not just a spectral shape) than anything else in this document's pipeline currently accepts.

#### 2.3.1 Human (*Homo sapiens*) — Trichromat (N=3)
- **S-cone**: λmax ≈ 420 nm. **M-cone**: λmax ≈ 530 nm. **L-cone**: λmax ≈ 560 nm.
- **Luminance weighting**: L+M dominant, consistent with the standard photopic luminosity function (CIE V(λ)); S-cone contribution near zero.
- **Opponent pairing**: literature-standard red-green (L−M) and blue-yellow (S−(L+M)) candidate contrasts, orthogonalized against luminance per §2.2.4 (both already nearly independent of luminance in the human case).

#### 2.3.2 Dog (*Canis lupus familiaris*) — Dichromat (N=2)
- **S-cone**: λmax ≈ 429–435 nm. **L/M-cone**: λmax ≈ 555 nm.
- **Visual system**: functional dichromat, analogous to a human blue-yellow-only system.
- **Luminance weighting**: uniform across both receptors (no dog-specific literature weighting located).
- **Opponent pairing**: single chroma channel (N-1=1); with N=2 the candidate-contrast/orthogonalization procedure reduces to a single S-vs-(L/M) contrast orthogonalized against luminance.

#### 2.3.3 Cat (*Felis catus*) — Trichromat by measured mechanism (N=3)
- Three photopic mechanisms at λmax ≈ 450, 500, and 556 nm.
- Modeled as N=3, including the 500nm mechanism in both luminance and chroma computation via the standard all-receptors-contribute procedure. Cats are frequently characterized behaviorally as functional dichromats, with the 500nm mechanism's contribution to color discrimination specifically debated in the literature — this is a disputed behavioral interpretation, not a retraction of the measured three-mechanism finding; the model uses the directly measured data.
- **Luminance weighting**: uniform across the three (no explicit literature weighting located).

#### 2.3.4 Honeybee (*Apis mellifera*) — Trichromat (N=3)
- **UV receptor**: λmax ≈ 344 nm. **Blue receptor**: λmax ≈ 436 nm. **Green/L receptor**: λmax ≈ 544–556 nm.
- **Luminance weighting**: weighted heavily toward the green/L receptor — honeybees use their long-wavelength receptor for both achromatic (luminance) and colour vision, with colour vision comparing responses of all three photoreceptors.
- **Opponent pairing**: literature-identified RNL-model opponent mechanisms used as candidates in the §2.2.4 procedure; all three receptors contribute to chroma.

#### 2.3.5 Pigeon (*Columba livia*) — Tetrachromat (N=4)
- **UV/Violet-sensitive (SWS1) opsin**: λmax ≈ 409 nm. **Short-wave (SWS2) opsin**: λmax ≈ 452–460 nm. **Medium-wave (RH2) opsin**: λmax ≈ 507–514 nm. **Long-wave (LWS) opsin**: λmax ≈ 567 nm.

**Oil droplet composite modeling**: each pigeon single-cone photoreceptor is modeled as a composite curve — the opsin absorption curve multiplied pointwise by that cone's oil droplet transmittance curve — rather than a single adjusted λmax. The oil droplet transmittance is stored as a `Transmittance`-type curve; the effective sensitivity curve is a derived curve (opsin × transmittance).

Oil droplets act as sharp short-wavelength cut-off filters. 50%-transmission cut-off wavelengths (red-field values used as the representative default; pigeons have two retinal fields — red and yellow — with different droplet populations, not modeled separately by default):
- Transparent/clear droplet (paired with UV/violet SWS1 cone): negligible filtering, effectively unity transmittance.
- "Greenish"/C-type droplet (paired with SWS2 cone): cut-off ≈ 430 nm.
- Yellow droplet (paired with RH2 cone): cut-off ≈ 470–480 nm.
- Orange/red droplet (paired with LWS cone): cut-off ≈ 550–610 nm depending on retinal field.

Effective sensitivity curve = opsin absorption curve (Govardovskii template at the given λmax) × oil droplet transmittance curve (sigmoid-like cut-off at the given 50%-transmission wavelength).

- **Luminance weighting**: uniform across all four (no explicit literature weighting located).
- **Opponent pairing**: no pigeon-specific literature opponent mechanism located; all four receptors use the generic per-receptor candidate contrast, orthogonalized against luminance to yield 3 independent chroma channels.

#### 2.3.6 Goldfish (*Carassius auratus*) — Tetrachromat (N=4)
- **UV receptor**: λmax ≈ 356 nm. **S/blue receptor**: λmax ≈ 455 nm. **M/green receptor**: λmax ≈ 530 nm. **L/red receptor**: λmax ≈ 625 nm.
- Goldfish use porphyropsin-based (vitamin A2) visual pigments; the A2 Govardovskii template variant is used for curve generation (a generation-time detail, not a stored field).
- **Luminance weighting**: uniform across all four (no explicit literature weighting located).
- **Opponent pairing**: no goldfish-specific literature opponent mechanism located; all four receptors use the generic per-receptor candidate contrast, orthogonalized against luminance to yield 3 independent chroma channels.

#### 2.3.7 Penguin — *Spheniscus humboldti* (Humboldt penguin) — Trichromat (N=3)
- **Violet-sensitive (SWS1)**: λmax ≈ 403 nm. **S/blue receptor**: λmax ≈ 450 nm. **L/green receptor**: λmax ≈ 543 nm.
- Unlike most birds (tetrachromats with a long-wave/red-sensitive receptor), penguins have lost both the UV-sensitive and long-wavelength-sensitive cone classes retained by most avian lineages, leaving three classes shifted toward shorter wavelengths — an evolutionary reduction reflecting adaptation to the aquatic environment, with vision adapted for good blue-green discrimination but poor long-wavelength discrimination.
- **Luminance weighting**: uniform across all three (no explicit literature weighting located).
- **Opponent pairing**: no penguin-specific literature opponent mechanism located; all three receptors use the generic per-receptor candidate contrast, orthogonalized against luminance to yield 2 independent chroma channels.

#### 2.3.8 Mouse (*Mus musculus*) — Dichromat (N=2)
- **S-cone (UV-sensitive)**: λmax ≈ 360 nm. **M-cone (green-sensitive)**: λmax ≈ 508 nm.
- **Visual system**: behaviorally-confirmed dichromatic color discrimination (unlike the more debated dog case, §2.3.2), though restricted in practice to the upper visual field — S-opsin-dominant cones are denser in the ventral retina (sampling the upper visual field via the inverted optical image), with a dorsal-ventral density gradient rather than uniform distribution across the retina. This retinal non-uniformity is a modeling simplification candidate: the default model treats the receptor curves as retina-wide averages rather than modeling spatial variation, consistent with how pigeon's two retinal fields (§2.3.5) are similarly collapsed to a single representative set by default.
- **Opsin coexpression**: a substantial fraction of mouse cones coexpress both S- and M-opsins within the same photoreceptor (rather than being cleanly segregated into separate S-only and M-only cells, as in most other species in this document). This does not change the two-receptor-type model structurally, since the perceptual pipeline operates on receptor sensitivity curves rather than individual cell identity, but is noted as a biological detail that distinguishes mouse from a "clean" dichromat like dog.
- **Luminance weighting**: uniform across both receptors (no explicit literature weighting located).
- **Opponent pairing**: single chroma channel (N-1=1); with N=2 the candidate-contrast/orthogonalization procedure reduces to a single S-vs-M contrast orthogonalized against luminance, analogous to dog's blue-yellow-only axis.

#### 2.3.9 Swallowtail Butterfly (*Papilio xuthus*) — Tetrachromat for Color Vision (N=4), with 2 Additional Non-Participating Receptor Classes Stored Separately
- **Color-vision receptors (N=4, full pipeline participation)**: **Ultraviolet (UV)**: λmax ≈ 360 nm. **Blue (B)**: λmax ≈ 460 nm. **Green (G)**: λmax ≈ 520 nm (reported with both single-peaked and double-peaked subtypes). **Red (R)**: λmax ≈ 600 nm. These four receptor classes are modeled as the species' Curve Set for all purposes in §2–§6: all four contribute to both luminance and chroma via the standard, exception-free §2.2.4 construction, consistent with every other species in this document.
- **Additional measured receptor classes, stored outside the perceptual pipeline**: the compound eye also contains two further spectrally distinct receptor classes — **Violet (V)**, λmax ≈ 400 nm (a narrow-band receptor derived from the same opsin as the UV receptor, spectrally narrowed by a UV-absorbing filtering pigment rather than a distinct opsin gene), and **Broad-band (BB)**, spanning a wide wavelength range rather than a narrow peak. These are documented in the literature as participating in non-chromatic visual functions (the broad-band receptor specifically is implicated in motion processing, with faster response dynamics than the color-vision-associated receptors) rather than color opponency.
- **Modeling decision**: per §2.2.4's exception-free rule (every receptor in a species' Curve Set contributes to both luminance and chroma, with no literature-based or otherwise evidenced exclusion permitted within that pipeline), the violet and broad-band curves are **not** included in the 4-receptor Curve Set used for the perceptual pipeline. Instead, they are stored as two additional Spectral Curves (§1.2.1, `Curve type = Sensitivity`) attached to the same species dataset for reference and completeness — preserving the full measured data — but explicitly outside the N=4 Curve Set that §2–§6's calculations operate on. This avoids weakening §2.2.4 into a rule with case-by-case exceptions, at the cost of the butterfly's default model representing color vision only, not the complete photoreceptor complement.
- **Luminance weighting**: not explicitly documented in the literature reviewed for the 4 color-vision receptors; defaults to uniform weighting, flagged as an approximation consistent with how other species lacking literature luminance data are treated (§2.3.2, §2.3.5–2.3.8).
- **Opponent pairing**: no butterfly-specific literature opponent-pairing mechanism (i.e. which receptors oppose which, analogous to human L−M) was located; the four color-vision receptors use the generic per-receptor candidate contrast (§2.2.4), orthogonalized against luminance to yield 3 independent chroma channels.




| Species | N | Receptor λmax (nm) | Luminance weighting |
|---|---|---|---|
| Human | 3 | 420, 530, 560 | Literature (L+M dominant, CIE V(λ)-equivalent) |
| Dog | 2 | 432 (avg), 555 | Uniform |
| Cat | 3 | 450, 500, 556 | Uniform; all 3 included in chroma |
| Honeybee | 3 | 344, 436, 550 (avg) | Literature (L-dominant) |
| Pigeon | 4 | 409, 456 (avg), 510 (avg), 567 (opsin peaks; effective peaks shift longer after oil-droplet filtering) | Uniform |
| Goldfish | 4 | 356, 455, 530, 625 | Uniform; A2 pigment template |
| Penguin (Humboldt) | 3 | 403, 450, 543 | Uniform; reduced from typical avian tetrachromacy |

All species use the generically-derived sharpening matrix (§2.2.3) for chromatic adaptation and the all-receptors-contribute candidate-contrast/orthogonalization construction (§2.2.4) for chroma channels — every receptor contributes to both luminance and chroma in every species' model.

---

## 3. Perceptual Coordinates, Difference Matrix, and Hyperspectral Image Extraction

### 3.1 Purpose
Given one or more input spectral curves and a target species' model, compute and display the perceptual coordinate vector for each, and a pairwise perceptual-difference matrix across all input spectra. Support extracting spectral curves by selecting regions of an imported multispectral/hyperspectral image.

### 3.2 Single-Spectrum Coordinate Calculation
Applying the §2.2 pipeline to a spectrum and species model produces:
```
[Luminance, Chroma_1, ..., Chroma_(N-1)]
```
**Display**: the raw coordinate vector only — no plot. Perceptual-space coordinates are consumed as data/table output rather than graphed.

### 3.3 Multi-Spectrum Coordinates and Difference Matrix

**Shared reference environment constraint**: all spectra included in a given coordinate table or difference matrix must be evaluated against the same reference environmental spectrum (§2.2.3). Perceptual coordinates are only comparable when they result from the same chromatic adaptation state. A single reference spectrum selection applies to an entire coordinate table / difference matrix computation, not per-spectrum.

#### 3.3.1 Coordinate table
| Spectrum | Luminance | Chroma₁ | Chroma₂ | ... | Chroma₍ₙ₋₁₎ |
|---|---|---|---|---|---|
| Spectrum A | ... | ... | ... | ... | ... |
| Spectrum B | ... | ... | ... | ... | ... |

Rows = input spectra (by name/label); columns = the species' perceptual coordinate dimensions.

#### 3.3.2 Perceptual difference matrix
A square, symmetric matrix of pairwise perceptual differences across input spectra, using a configurable distance metric selected per analysis. Three selectable presets:

1. **Euclidean**: `d(A,B) = sqrt( (L_A−L_B)² + Σ(C_i,A − C_i,B)² )`.
2. **Chroma-only**: `d(A,B) = sqrt( Σ(C_i,A − C_i,B)² )`, excluding luminance.
3. **Vorobyev–Osorio ΔS** (receptor-noise-limited, JND-style): a perceptually-calibrated distance in just-noticeable-difference units, operating on adapted receptor activations `Q'` directly (a parallel calculation path from the same adapted activations, not a transformation of the Euclidean/chroma-only outputs).
   - **Formula**: per-receptor channel noise `e_i = ω / sqrt(η_i)`, where `ω` is the Weber fraction and `η_i` is relative receptor density. Receptor types with higher relative abundance are modeled as less noisy.
   - **Weber fraction**: commonly assumed from literature convention where not directly measured (values of 0.05–0.2 appear across studies; 0.05 for the most abundant channel is a common convention in avian studies).
   - **Relative receptor density data by species**: honeybee and pigeon have literature-sourced relative cone abundance data usable for `η_i` (see References); dog, cat, goldfish, and penguin default to uniform relative density across receptor types (`η_i` equal for all `i`), a documented simplification flagged in the UI whenever ΔS is used for these species, noting it does not yet reflect actual retinal photoreceptor proportions.

**Output format**: an N×N symmetric matrix (N = number of input spectra), diagonal zero, displayed as a table.

#### 3.3.3 Batch export to file
The interactive coordinate table and difference matrix above don't scale to a corpus of thousands of input spectra (e.g. a bulk pixel-import batch, §4.2.6) - an egui table that size would be unusable to render and scroll. A separate "batch export" action writes a CSV file instead: one row per currently-selected input spectrum, with each selected species' luminance/chroma/saturation as that species' own block of columns (so species of different receptor count N just get different-width blocks in the same file). Optionally, each row also gets a "distance to reference" column per species - the pairwise metric above evaluated against one chosen reference spectrum, rather than the full N×N matrix (which stays impractical at this scale regardless of output format). Uses the same reference illuminant, species selection, and distance metric already chosen for the interactive view.

### 3.4 Hyperspectral/Multispectral Image Import and Region Extraction

#### 3.4.1 Supported formats
1. **ENVI format** (`.hdr` header + `.dat`/`.img`/`.raw` binary pair): the header specifies image dimensions (`samples`, `lines`, `bands`), per-band wavelength list, data type, and interleave scheme (`BSQ`, `BIL`, or `BIP` — all three supported). Band wavelength metadata from the header populates each extracted spectrum's wavelength axis directly, rather than assuming fixed/uniform band spacing.
2. **GeoTIFF (multiband)**: reads whatever wavelength tags are present - baseline TIFF has no such convention, so in practice this is always absent. The import flow then presents a dropdown of common sensor/instrument band-set presets, with a manual entry fallback for instruments not covered.
3. **MATLAB `.mat` hypercube**: import inspects the file's variable names and shapes, auto-guessing the mapping using shape heuristics (a 3D array proposed as the data cube; a 1D array whose length matches the cube's band dimension proposed as the wavelength vector), pre-filling a confirmation dropdown listing all variables found. The user can override either selection before import completes.

**Implementation note** (§10.3): ENVI reading is a hand-written header/BSQ-BIL-BIP parser (`envi.rs`); GeoTIFF reading uses the pure-Rust `tiff` crate (`geotiff.rs`), restricted to chunky (pixel-interleaved) storage - the overwhelmingly common case in practice, with planar storage rejected as an explicit error rather than silently misread. MATLAB `.mat` reading uses a separate dedicated crate (`matfile`). None of the three links a native library.

#### 3.4.2 Region selection and spectrum extraction
- **Interaction model**: user selects a region of the displayed image (rectangle, polygon/lasso).
- **Extraction method**: region selection produces one representative spectrum per region via spatial averaging — the mean value at each band across all pixels within the selected region.
- **Output**: the averaged spectrum is created as a new Spectral Curve (§1.2.1), auto-labeled (e.g. with source image filename and region identifier/coordinates), and added to the active Curve Set.
- **Multiple regions**: the user may select and extract multiple regions from the same or different images in sequence, building a set of curves for multi-spectrum comparison directly from image data.

---

## 4. Storing and Editing Receptor Noise and Opponent-Process Data

### 4.1 Purpose
Extend the data model so that receptor noise parameters (required for the Vorobyev-Osorio ΔS metric, §3.3.2), luminance weighting, and opponent-process pairing data can be stored and ergonomically edited alongside spectral response curves, for arbitrary visual systems.

### 4.2 Data Model Extension

#### 4.2.1 Weber fraction (`ω`) — per-curve field
An optional numeric field on the Spectral Curve object, alongside the existing curve type/name/metadata fields. A curve with no `ω` set has no noise data defined.

#### 4.2.2 Relative receptor density (`η`) — per-curve field, set-scoped meaning
`η` is only meaningful relative to the other receptor curves within the same Curve Set. Stored as a numeric field on each Spectral Curve (travels with the curve, editable per-row in the noise table, included in the file format), but its semantic validity is a property of the set as a whole.

- **Storage format**: raw literature values as transcribed — absolute counts or simple relative ratios (e.g. "1 : 1 : 2 : 2" entered as individual per-curve numbers) — stored exactly as entered, not pre-normalized.
- **Normalization**: performed automatically at calculation time, dividing each curve's `η` by the sum of `η` across all receptor curves in the set that have noise data defined.
- **Validation**: a Curve Set used for ΔS calculations should have `η` values defined on either all of its receptor curves or none — partial coverage is flagged to the user as producing an incomplete/unreliable noise model.

#### 4.2.3 Luminance weight (`w_i`) — per-curve field
An optional numeric field on the Spectral Curve, alongside `ω` and `η`. **Default when unset**: the curve's own integral (total area under the sensitivity curve), normalized against the other curves in the set, giving an unknown/custom receptor a non-arbitrary default. **Explicit override**: where literature specifies luminance weighting that is not proportional to curve integral (e.g. human L+M-dominant luminance despite S having nonzero integral; honeybee L-receptor-dominant luminance), `w_i` is set explicitly and takes precedence — luminance weighting reflects downstream neural wiring/routing, not merely the receptor's raw spectral sensitivity shape, and the two can diverge.

#### 4.2.4 Opponent contrast definition — per-set field
Literature-defined opponent pairings (e.g. human L−M and S−(L+M); honeybee RNL-model mechanisms) are stored as explicit structured data on the Curve Set: a list of named contrasts, each specifying which curves contribute positively/negatively and with what weights (a stored representation of the `Cand_i` construction from §2.2.4). When a Curve Set has no stored opponent contrast definitions, the generic per-receptor "candidate vs. mean of others" formula (§2.2.4) is used as the fallback at calculation time.

#### 4.2.5 Data-quality / provenance notes
Provenance information (literature-grounded vs. approximated, with citation) is recorded as free-form text within the existing optional metadata (§1.2.1) on the relevant curve or set, not as a separate structured field.

#### 4.2.6 Automatic natural-scene-derived opponent contrasts
Human L−M and S−(L+M) are themselves a fixed, evolutionarily-cheap approximation to a solution that is actually scene-statistics-dependent: the principal components of cone responses to natural scenes track those two channels closely (Buchsbaum & Gottschalk 1983; Ruderman, Cronin & Chiao 1998; Wachtler, Lee & Sejnowski 2001). As an alternative to literature entry or the generic fallback (§4.2.4), opponent contrasts can be derived automatically from that same kind of statistics, for any species' receptor set:

1. Build the receptor-response covariance matrix for the set's colorspace curves under a chosen reference illuminant, from one of two "natural scene" ensembles:
   - **Parametric model**: no external data. Natural reflectance spectra are modeled as a smooth random process in wavelength, with an exponential autocorrelation kernel `K(λ,λ') = exp(-|λ-λ'|/ℓ)` (motivated by the established finding that natural/Munsell reflectances are smooth and low-dimensional, e.g. Maloney 1986). With `T_i(λ) = S_i(λ)·E(λ)` (receptor `i`'s sensitivity weighted by the illuminant `E`), the covariance is the exact quadratic form `Cov[i,j] = T_i · K · T_jᵗ` — no sampling, no embedded dataset. Correlation length `ℓ` is a user-adjustable, tunable approximation, not a measured literature constant.
   - **Custom corpus**: a user-supplied weighted set of reflectance/radiance curves (e.g. imported from a USGS spectral library, or in bulk from every pixel of a loaded multispectral image — see below), each resolved against the illuminant the same way a stimulus curve is elsewhere (§5.3's reflectance×illuminant rule), then combined into the weighted empirical covariance of their receptor-response vectors.

   A single illuminant spectrum alone cannot supply either ensemble: it rescales/tints responses but contributes no *variance across stimuli* for the next step to decorrelate — the variance has to come from the ensemble of surfaces the illuminant is applied to.
2. Eigendecompose the covariance matrix. Each eigenvector becomes one candidate opponent contrast row (same shape as `generic_candidate_rows`/a literature-defined row — one per receptor), ordered by descending eigenvalue (most natural-scene variance first). The highest-variance component tends to be luminance-like (same sign across all receptors) for broadly-overlapping receptor sets, consistent with the literature findings above; `build_chroma_axes`' own orthogonalization against the (separately-defined) luminance direction reduces its contribution the same way it would for any other candidate row, with no special-casing needed.

Generating from either ensemble replaces the Curve Set's `opponent_contrasts` outright.

**Relationship to §2.2.3**: this is a deliberately different design choice from `M_adapt`'s derivation, which avoids any natural-corpus dependency specifically so adaptation stays a property of the receptors alone, not an artifact of dataset selection. Opponent contrasts are different: the literature claim being modeled here (Buchsbaum/Ruderman/Wachtler) is specifically *about* natural-scene statistics, so corpus-dependence is the point, not something to avoid.

**Bulk corpus-building tools**: to make the custom-corpus ensemble practical at more than a handful of hand-picked curves, the Stimulus Editor (§3.4) offers two bulk operations alongside its existing single-curve ones:
- **Import every pixel as a curve**: the §3.4.2 region-extraction mechanism's "entire pixel population" counterpart to its single spatial average — one radiance curve per pixel in a region (or the whole image), keeping every *N*th pixel via an adjustable stride. Each curve is tagged with a corpus path (below) nesting it under its source image and region, so the corpus picker and other stimulus-library views can select or display the whole group as one unit instead of one row per pixel.
- **Bulk radiance → reflectance**: the single-curve "derive reflectance from radiance ÷ luminant" operation (§5.3.1), applied to many selected curves (e.g. a whole pixel-import group) against one shared chosen luminant at once, producing a group of new reflectance curves alongside the originals, nested as a sub-corpus of the group converted from.

**Corpora and sub-corpora**: any stimulus curve can be placed into a corpus/sub-corpus hierarchy via a `"/"`-separated path (e.g. `"forest.tif/Canopy"`), stored as the curve's own metadata and editable directly in the Stimulus Editor (blank = ungrouped, same as before this existed). Every picker that lists the shared stimulus library - this one, the bulk tools above, and Comparison's own stimulus checklist - renders the hierarchy as nested, collapsible groups, each with its own "select everything under this corpus" checkbox, so a whole corpus (or a whole sub-corpus) can be included or excluded in one click regardless of how many curves it contains.

### 4.3 Editing

Receptor noise and luminance-weight data are edited via a dedicated numeric table, one row per receptor curve in the active Curve Set, with columns for `ω`, `η`, and `w_i` (displaying the integral-derived default when unset, editable to override). This table is presented alongside the per-curve point editor from §1.5 rather than embedded within it, since these are single scalar values per curve, and benefit from being viewed across all receptors in the set simultaneously.

Opponent contrast definitions (§4.2.4) are edited via a separate set-level view — a list of named contrasts, each letting the user pick contributing curves and their signed weights, with an option to clear a contrast definition back to the generic per-receptor fallback. The same view offers the §4.2.6 automatic-generation control alongside manual entry: choose a parametric model or a custom corpus and a reference illuminant, then generate, replacing the table in one step.

- Editing a cell updates that curve's `ω`, `η`, or `w_i` value directly.
- Rows with no noise data or luminance weight entered display as empty/blank rather than a default value, distinguishing "no data available" from "a measured value of zero" — except `w_i`, which always displays its computed integral-based default rather than blank.
- The table surfaces the validation note from §4.2.2 (partial `η` coverage within a set) inline.

### 4.4 Applicability to Default Species Models
Honeybee and pigeon's default receptor curves ship with `ω`/`η` values pre-populated from cited literature; the other four species' default curves ship with these fields left empty, surfacing the "incomplete coverage" flag by default until a user supplies values via the editing table.

---

## 5. Illumination Curve Acquisition and Generation

### 5.1 Purpose
Extend curve-acquisition and derivation capabilities to illumination (reference environmental spectrum) curves: extracting them from hyperspectral image regions, back-deriving illuminant-independent reflectance from a measured sample, converting that reflectance to its predicted appearance under a different illuminant, and generating synthetic illumination curves parametrically.

### 5.2 Extracting Illumination Curves from Image Regions
Identical mechanism to §3.4.2 (region select + spatial average → new curve), with the resulting curve tagged `Curve type = Illumination` rather than treated as a reflectance sample.

### 5.3 Back-Deriving Illuminant-Independent Reflectance

#### 5.3.1 Derivation
Given a measured curve `M(λ)` and the known illuminant curve `I(λ)` it was measured under (defaulting to daytime Earth solar illumination, §5.4, if unspecified):
```
Reflectance(λ) = M(λ) / I(λ)
```
The result is normalized to represent a true physical 0–1 reflectance fraction, requiring `M(λ)` and `I(λ)` to be in consistent, compatible radiometric units before division.

**Unit consistency enforcement**: this operation is a hard block, using the quantity kind/unit metadata (§1.2.1):
- Both `M(λ)` and `I(λ)` must have a quantity kind of `Radiance` or `Irradiance` (not `Unspecified`, and not an already-dimensionless kind like `Reflectance`) with matching units before the operation is permitted.
- If either curve's quantity kind is `Unspecified`, the operation is refused and the user is prompted to specify the missing quantity kind/unit before proceeding.
- If both curves have a quantity kind specified but units are mismatched, the operation is likewise refused, with the user prompted to reconcile before the division proceeds.
- This hard-block behavior is specific to this operation because an undetected unit mismatch would silently produce a physically meaningless reflectance curve that propagates into every downstream calculation that consumes it (§5.3.2, and the full §2 perceptual pipeline).

The resulting curve is stored as a new Spectral Curve, tagged `Curve type = Reflectance` (quantity kind `Reflectance`, unitless by definition).

#### 5.3.2 Converting to a Different Illuminant
```
Predicted_measured(λ) = Reflectance(λ) × I_new(λ)
```
The same physical relationship run forward — not an approximation, provided §5.3.1's unit-consistency requirement was satisfied. Produces a new derived curve representing the expected measured signal under the new illuminant, stored as a new Spectral Curve tagged consistently with its role (distinguishable from directly-measured data via metadata).

### 5.4 Default Illuminant: Daytime Earth Solar
The default illuminant for §5.3's division, when no specific illuminant curve is supplied, is a standard daytime Earth solar spectrum, provided as a built-in default curve (e.g. following ASTM G173 or CIE D65 daylight reference conventions), available out-of-the-box. A 5778K black-body curve (§5.5.1) approximates the Sun's photospheric output reasonably well as a parametrically-generated alternative.

### 5.5 Synthetic Illuminant Generation

Both generation modes below produce ordinary Spectral Curves (tagged `Curve type = Illumination`) using the existing arbitrary-point curve data model — no new curve representation is introduced.

#### 5.5.1 Black-Body Generation
**Formula** (Planck's law):
```
L(λ, T) = (2hc² / λ⁵) · 1 / (e^(hc/λkT) − 1)
```
where `h` = Planck's constant, `c` = speed of light, `k` = Boltzmann's constant, `T` = absolute temperature in Kelvin (user-specified). The generator evaluates this formula across the user's chosen wavelength range, then normalizes appropriately for use as an illuminant curve.

**Gaseous absorption cut-outs** (both methods available, applied as multiplicative attenuation on the black-body curve):
1. **Preset atmospheric absorption bands** (toggleable):
   - Molecular oxygen (O₂) A-band: centered near 759–762 nm.
   - Molecular oxygen (O₂) B-band: centered near 686–688 nm.
   - Water vapor bands: near-infrared absorption centered approximately at 720, 820, 940, 1100, 1130, 1370, 1450, 1950, and 2500 nm (reported sets vary slightly by source but reflect the same underlying absorption structure); a weaker visible/near-UV water vapor feature is available as a lower-confidence optional entry.
   - Extensible list — additional bands (e.g. CO₂ absorption near 1400, 1600, 2000 nm) can be added following the same sourcing approach.
2. **Manual notches**: arbitrary absorption notches specified by center wavelength, depth, and width, for cases not covered by presets (e.g. non-Earth atmospheres).

The combined result (black-body × preset bands × manual notches) is stored as the final curve's point data, at which point it is an ordinary curve editable via §1.5's point editor.

#### 5.5.2 Composite Narrow-Band Source Generation
Three complementary input methods feeding into the same underlying curve:

1. **Preset library of named real-world sources**:
   - **Low-pressure sodium vapor**: two closely-spaced emission lines at 589.0 nm and 589.6 nm (sodium D-lines), negligible output elsewhere in the visible range. Modeled as two narrow Gaussian peaks.
   - **High-pressure sodium vapor**: pressure-broadened emission around the 589 nm region, with additional emission above and below it. Modeled as a broader peak/envelope centered near 589 nm.
   - **Mercury vapor**: discrete spectral lines at approximately 365, 405, 436, 546, and 578 nm. Modeled as narrow Gaussian peaks at these wavelengths with relative intensities per published mercury-lamp emission data.
   - **Cool white LED / Warm white LED (phosphor-converted)**: a narrow blue pump-chip peak near 450 nm plus a broad phosphor-emission hump spanning roughly 500–700 nm (peak typically 550–580 nm). Cool vs. warm variants are distinguished by the relative weighting between the blue peak and the phosphor hump (warm white weights the longer-wavelength contribution more heavily). Modeled as a narrow Gaussian (blue peak) plus a broader Gaussian/asymmetric hump (phosphor band).
   - Extensible list — additional named sources (metal halide, fluorescent, specific research light sources) can be added following the same approach.
2. **Gaussian peak tool**: specify center wavelength, peak intensity, and bandwidth to add a Gaussian-shaped spectral peak — the same underlying mechanism used to construct each preset above, so presets are pre-configured sets of Gaussian peak parameters rather than a separately-implemented curve type.
3. **Direct point editing**: once presets and/or Gaussian peaks have been added, the result is ordinary point data in the existing curve editor (§1.5), freely hand-editable.

**Combining multiple components**: multiple narrow-band components are combined via weighted summation — each component contributes a user-specified relative-intensity weight before being summed pointwise into the composite curve, rather than simple unweighted addition. This allows, for example, a dominant "Cool white LED" preset combined with a weaker secondary sodium-vapor contribution to model mixed-lighting scenes.

---

## 6. Cross-Species Distinguishability Comparison

### 6.1 Purpose
Allow directly comparing how distinguishable a given set of stimuli are to different species.

### 6.2 Cross-Species Comparability
Perceptual distance values produced by §3.3.2 are not inherently comparable across species: different receptor counts (different coordinate-space dimensionality) and different adaptation/sharpening matrices (§2.2.3) mean Euclidean and chroma-only distance have no defined relationship across species' spaces.

**Metric**: cross-species comparison uses **Vorobyev-Osorio ΔS (JND units)** as the primary metric, since it is calibrated to each species' own discrimination threshold — a ΔS value represents "distance in just-noticeable-differences," the closest available common currency for "can this species tell these stimuli apart." Euclidean and chroma-only distances remain available per-species but are explicitly labeled as not comparable across species wherever shown in a cross-species context.

**Caveat**: cross-species ΔS comparison is the best available approximation, not an exact equivalence — "1 JND" is not guaranteed to represent an identical underlying perceptual experience across species, and reliability depends on the underlying Weber fraction/receptor density data (§3.3.2, §4) for each species involved. This caveat is surfaced per-species in the comparison output, since data quality varies species-by-species.

### 6.3 Presentation

#### 6.3.1 Per-species difference matrices
For N input stimuli, a ΔS difference matrix (§3.3.2's format) is computed independently for each selected species, using that species' own full pipeline (receptor model, adaptation against the shared reference environment per §3.3, noise parameters per §4).

#### 6.3.2 Cross-species summary view
A summary table presents the same stimulus-pair comparisons side by side across species:

| Stimulus Pair | Human ΔS | Dog ΔS | Cat ΔS | Honeybee ΔS | Pigeon ΔS | Goldfish ΔS | Penguin ΔS |
|---|---|---|---|---|---|---|---|
| A vs. B | ... | ... | ... | ... | ... | ... | ... |
| A vs. C | ... | ... | ... | ... | ... | ... | ... |

- Rows = stimulus pairs; columns = species included in the comparison (user-selectable subset).
- **Data-quality flag**: every species' column includes a visible indicator of its noise-data provenance — literature-grounded (honeybee, pigeon, per current defaults) vs. approximated/uniform-density (dog, cat, goldfish, penguin, per current defaults) — surfaced inline (e.g. an icon or annotation per cell or column header). All species are included regardless of data quality tier; none are excluded by default.

### 6.4 Shared Reference Environment Constraint
Inherited unchanged from §3.3: all species being compared must evaluate the same input stimuli against the same reference environmental spectrum for the comparison to be meaningful.

---

## 7. Custom Visual Systems of Arbitrary Dimension

### 7.1 Purpose
The application supports defining entirely custom visual systems, not limited to the 10 default species/system models (§2.3) — of arbitrary receptor count N. The 10 models are default/example models, not an exhaustive or closed list. Any Curve Set consisting of N receptor-type Spectral Curves is a valid input to the full perceptual pipeline (§2), coordinate/difference-matrix tools (§3), noise-data storage (§4), and cross-species comparison (§6) — regardless of whether it corresponds to a preset, a different real species, or a hypothetical/fictional visual system.

### 7.2 Receptor Count (N) Bounds

- **Minimum**: N ≥ 1. A single-receptor (monochromatic) visual system is valid. At N=1, §2.2.4's construction degrades gracefully: the one candidate opponent contrast coincides with the luminance direction itself (since `L = w_1·Q'_1` is just the single receptor's weighted activation), so orthogonalizing it against luminance leaves zero independent chroma channels — the physically correct result for a monochromat (luminance perception only, no chroma).
- **Maximum**: no hard upper bound. A soft practical cap with a warning applies — very high N is permitted but flagged as likely slow to compute (particularly the §2.2.3 adaptation-matrix optimization, which scales with N) and difficult to interpret. The specific cap threshold is an implementation-tuning detail, to be determined based on actual performance characteristics of the adaptation-matrix derivation.

### 7.3 Creation Workflow
A guided wizard flow: the user specifies N upfront; the application generates N blank receptor-type Spectral Curve slots within a new Curve Set, ready for population via the existing curve point editor (§1.5). This front-loads the one structural decision that determines the shape of everything downstream (chroma dimensionality, table/matrix column counts in §3.3 and §6.3).

After the N receptor curve slots are created:
- **Receptor sensitivity curves**: populated via §1.5's point editor.
- **Noise parameters** (`ω`, `η`, §4.3): optional, populated via the dedicated noise table.
- **Luminance weighting** (`w_i`, §4.2.3): optional; defaults to the curve-integral-derived weight unless the user supplies custom weights.
- **Opponent-pairing candidates** (§4.2.4): defaults to the generic "receptor vs. mean of others" candidate per receptor, since a user-defined custom system has no literature pairing to draw on by definition.

### 7.4 Interaction with Other Sections
No other section requires modification to support custom systems — §2's pipeline, §2.2.3's generic adaptation-matrix derivation, and §2.2.4's all-receptors-contribute construction operate on N as a free parameter; §3 operates on "a species' model," which a custom system satisfies identically to a preset; §4 attaches to Spectral Curves and Curve Sets generically; §6 operates on "selected species models" without assuming a fixed roster.

---

## 8. Representability and Fixture-Based Packaging of the 7 Preset Species

### 8.1 Purpose
Guarantee that every piece of data used by the 7 default species models (§2.3) is representable in the generic Curve Set / Spectral Curve data model, and that the 7 species' data is shipped as external data fixtures rather than hardcoded into the application.

### 8.2 Representability

All data used by the 7 default species models is representable in the generic data model:

| Data point | Representation |
|---|---|
| Receptor sensitivity curve points | §1.2.1 curve points |
| Curve type / quantity kind | §1.2.1 |
| Species/set name | §1.2.2 |
| Weber fraction (`ω`) | §4.2.1 |
| Relative receptor density (`η`) | §4.2.2 |
| Luminance weight (`w_i`) | §4.2.3 — per-curve field, curve-integral-derived default, explicitly overridable |
| Opponent contrast definitions | §4.2.4 — per-set field storing literature-known pairings as explicit structured data, falling back to the generic per-receptor formula when absent |
| Oil droplet transmittance (pigeon) | §2.3.5 — a second composited curve |

| Literature citations / provenance | §1.2.1 optional metadata (free-form) |
| A1/A2 pigment template choice (goldfish) | Generation-time modeling detail, not a stored field — the resulting curve is ordinary point data regardless of which template produced it |

No species-specific behavior depends on code paths unavailable to a user-defined custom system (§7).

### 8.3 Fixture Packaging

The 7 species models are shipped as **7 separate files**, one per species, each a serialized Curve Set using the file format defined in §1.3 (including §4's noise data and §4.2.3/§4.2.4 extensions) — not combined into a single file, and not represented as in-code data structures, constants, or hardcoded initialization logic anywhere in the application.

**Runtime treatment**: these 7 files are placed in a known, user-visible location (e.g. an application data directory) as ordinary, user-editable files — not read-only bundled resources. A user can open, inspect, modify, or delete them exactly as they would any other saved Curve Set, using the mechanisms in §1.3/§1.5. This means the 7 presets are simultaneously (a) working examples a new user can learn the data format from by inspection, and (b) ordinary starting points a user can freely customize, with no separate "preset vs. custom" code path.

**Recovery**: because these files are user-editable and can be modified, corrupted, or deleted, the application retains an internal pristine copy of all 7 species fixtures (embedded as read-only reference data at build time, separate from the user-visible/editable copies on disk) and exposes a "restore defaults" / "re-extract fixtures" action that regenerates the 7 user-visible files from this pristine copy.

### 8.4 Interaction with Other Sections
§7's custom visual systems and the 7 preset species are the same kind of object at the data level: both the representability guarantee (§8.2) and the packaging decision (§8.3 — presets are editable like any other Curve Set) establish this directly.

---

## 9. "Save As" for Deriving a New Visual System

### 9.1 Purpose
Allow a new Curve Set to be derived from any existing one — preset or custom — without modifying or overwriting the original, supporting the workflow of starting from a known visual system and adjusting it into a variant (e.g. a hypothetical mutant, a different individual's measured data, or a simplified teaching example).

### 9.2 Mechanism
An explicit "Save As" action, available from any open Curve Set. Invoking it prompts for a new name, then writes the Curve Set's full current state to a new file (§1.3's format) under that name, leaving the original file untouched on disk. The newly saved copy becomes the active Curve Set in the editor; the original remains available to be separately reopened unmodified.

This is an explicit action rather than automatic fork-on-edit, so the user always knows which file is currently being edited and when a new one has been created.

### 9.3 Scope
Save As applies uniformly to both the default preset species/systems (§2.3, §8.3) and user-created custom visual systems (§7), consistent with §7/§8's establishment that presets and custom systems are the same kind of object at the data level. This includes forking either of frog's two regime-specific Curve Sets (§2.3.10) independently — e.g. a user removing the contested middle cone from the default photopic frog model (per §2.3.10's documented note) would do so via Save As, producing a new, independently-named Curve Set without modifying the shipped default.

**Relationship to §8.3's fixture recovery**: Save As is distinct from "restore defaults." Editing a preset's file directly and then using "restore defaults" reverts that preset to its pristine shipped state; using Save As on a preset instead creates an independent new file, leaving the preset's original file untouched throughout — a user who forks a preset via Save As does not need "restore defaults" to recover the original, since it was never modified.

### 9.4 Copy Semantics
Save As performs a full deep copy of the Curve Set's complete current state into the new file — all receptor curves and their point data, all per-curve fields (`ω`, `η`, `w_i`, quantity kind, metadata), and any set-level opponent contrast definitions — duplicated unchanged into the new Curve Set. Nothing is reset to defaults on fork. Subsequent edits to either copy do not affect the other.

---

## 10. Open TODOs (Future Work, Not Yet Implemented)

Recorded for future scheduling; not yet designed in further detail or implemented.

### 10.1 Display Individual Photoreceptor Activations — IMPLEMENTED
~~Display each receptor's individual raw activation value (§2.2.2's Step 1 output, before adaptation/luminance/chroma are derived from it), including activations for curves held in `reference_curves` (§1.2.2)~~ — implemented in Perceptual Comparison's coordinate output: `Pipeline::colorspace_activations`/`isolated_activations` compute `Q_i = ∫ S_i(λ)·stimulus(λ) dλ` identically for the colorspace set and for the isolated curves (neither is adapted at this stage, so the same formula applies to both without needing to decide how an isolated curve would be "adapted"), displayed as additional columns per included stimulus, with isolated-curve columns labeled `(isolated) <name>` to distinguish them from colorspace ones. (`CurveSet`'s two curve lists were later renamed from `curves`/`reference_curves` to `colorspace_curves`/`isolated_curves` for clarity - see §1.2.2.)

### 10.2 Mantis Shrimp — All-Isolated-Curves Visual System — IMPLEMENTED
~~Add a Mantis Shrimp (Stomatopoda) species model whose defining characteristic is that **none** of its photoreceptors participate in opponent-process colorspace construction~~ — implemented as a 13th built-in fixture: 12 photoreceptor classes (illustrative λmax values spanning UV to red), all isolated, with an **empty** colorspace set — the first fixture with zero colorspace receptors, exercising `derive_adaptation_matrix`'s `n == 0` case (a trivial 0×0 identity, the same reasoning as its `n == 1` case). Every other part of `Pipeline` (candidate rows, chroma axis construction, `coordinates`) handles N=0 correctly without further special-casing, matching §8.2's representability claim even for this edge case.

### 10.3 Replace GDAL with a Pure-Rust Implementation — IMPLEMENTED
~~§3.4.1 implemented ENVI and GeoTIFF reading via the `georust/gdal` crate, which brought in GDAL as a large native C/C++ library dependency rather than a pure-Rust one - a cost specific to this app's cross-platform-desktop-application requirement (§1 implies this explicitly): GDAL has no Linux-`pkg-config`-level convenience on Windows (typically vcpkg/conda-forge/OSGeo4W instead), and the GDAL shared library had to be present at runtime on every machine the app shipped to, not just at build time.~~ — replaced by a hand-written ENVI header/BSQ-BIL-BIP parser (`envi.rs`) and the pure-Rust `tiff` crate for GeoTIFF (`geotiff.rs`), with GDAL kept only as a dev-dependency for one cross-validation test (confirming the GeoTIFF reader opens real GDAL-written files identically, not just its own fixtures). The resulting release binary links nothing beyond the C library every Rust binary already needs (`libc`/`libgcc_s`/`libm`), confirmed via `ldd` - no PROJ/GEOS/GDAL, and no per-platform GDAL install step in `BUILDING.md` anymore.

### 10.4 Comprehensive UI/UX Review
A dedicated pass over the whole application's user interface and user experience, as opposed to the feature-by-feature, section-by-section review this document's development has used so far (each phase's UI landing alongside its own calculation logic, reviewed in isolation). Scope not yet defined - likely candidates include overall information architecture/navigation now that there are eight functional sections, consistency of terminology and interaction patterns across sections that were built at different times (e.g. "+Add"/"Remove" button phrasing, selector-row vs. checkbox-list conventions, status-message placement), discoverability of features that currently require scrolling through a long single-page layout, and a pass for plain-language clarity consistent with the earlier "build for the user, not an implementation auditor" feedback. Not scheduled; recorded as a known gap rather than assumed to be covered by the per-section reviews already done.

---

## References

Literature and technical sources informing this specification, organized by section.

### Species Receptor Sensitivity Data (§2.3, §8.2)

**Human**
- Schnapf, J., Kraft, T., & Baylor, D. (1987). Spectral sensitivity of human cone photoreceptors. *Nature*, 325, 439–441.
- Human Cone Action Spectra (reference compilation of cone peak sensitivities, ~420/530/560 nm).

**Dog**
- Neitz, J., Geist, T., & Jacobs, G. H. (1989). Color vision in the dog. *Visual Neuroscience*, 3, 119–125.
- Jacobs, G. H., Deegan, J. F., Crognale, M. A., & Fenwick, J. A. (1993). Photopigments of dogs and foxes and their implications for canid vision. *Visual Neuroscience*, 10, 173–180.

**Cat**
- Cone contributions to cat retinal ganglion cell receptive fields (peak sensitivities at 450, 500, 556 nm).
- Trichromatic Vision in the Cat. *Science*.

**Honeybee**
- Menzel, R., & Backhaus, W. (1991). Colour vision in insects. In *Vision and Visual Dysfunction*, Vol. 6.
- Vorobyev, M., & Osorio, D. (1998). Receptor noise as a determinant of colour thresholds. *Proceedings of the Royal Society B*, 265, 351–358.

**Pigeon**
- Bowmaker, J. K. (1977). The visual pigments, oil droplets and spectral sensitivity of the pigeon. *Vision Research*, 17(10), 1129–1138.
- Bowmaker, J. K., Heath, L. A., Wilkie, S. E., & Hunt, D. M. (1997). Visual pigments and oil droplets from six classes of photoreceptor in the retinas of birds. *Vision Research*, 37(16), 2183–2194.
- Martin, G. R., & Muntz, W. R. (1978). Spectral sensitivity of the red and yellow oil droplet fields of the pigeon (*Columba livia*). *Nature*, 274, 620–621.
- Oil Droplet Distribution and Colour Discrimination in the Pigeon. *Nature New Biology*.

**Goldfish**
- Palacios, A. G., Goldsmith, T. H., & Bernard, G. D. (1998). Sensitivity of cones from the goldfish *Carassius auratus* to ultraviolet and visible light. *Visual Neuroscience*.
- Hárosi, F. I. (1976). Spectral relations of cone photoreceptors in goldfish retina (foundational microspectrophotometry studies).

**Penguin**
- Bowmaker, J. K., & Martin, G. R. (1985). Visual pigments and oil droplets in the penguin, *Spheniscus humboldti*. *Journal of Comparative Physiology A*, 156, 71–77.

**Mouse**
- Jacobs, G. H., Neitz, J., & Deegan, J. F. (1991). Retinal receptors in rodents maximally sensitive to ultraviolet light. *Nature*, 353, 655–656.
- Deegan, J. F., & Jacobs, G. H. (1993). On the identity of the cone types of the mouse retina.
- Applebury, M. L., et al. — cone pigment coexpression and spectral tuning studies informing the mouse green cone pigment absorption maximum (~508 nm).
- Natural image statistics for mouse vision (PLOS One) — modeled mouse visual system spectral sensitivity using S/M opsin templates centered at 360/508 nm.
- Neural circuits in the mouse retina support color vision in the upper visual field (*Nature Communications*) — behavioral dichromatic color discrimination and retinal density-gradient findings.

**Swallowtail butterfly (*Papilio xuthus*)**
- Arikawa, K., Mizuno, S., Scholten, D. G. W., Kinoshita, M., Seki, T., Kitamoto, J., & Stavenga, D. G. (1999). An ultraviolet absorbing pigment causes a narrow-band violet receptor and a single-peaked green receptor in the eye of the butterfly Papilio. *Vision Research*, 39, 1–8.
- Arikawa, K., Scholten, D. G. W., Kinoshita, M., & Stavenga, D. G. (1999). Tuning of photoreceptor spectral sensitivities by red and yellow pigments in the butterfly Papilio xuthus. *Zoological Science*, 16(1), 17–24.
- Koshitaka, H., Kinoshita, M., Vorobyev, M., & Arikawa, K. (2008). Tetrachromacy in a butterfly that has eight varieties of spectral receptors. *Proceedings of the Royal Society B*.
- Arikawa, K. (2003). Spectral organization of the eye of a butterfly, Papilio. *Journal of Comparative Physiology A*.
- Kawasaki, M., Kinoshita, M., Weckström, M., & Arikawa, K. (2015). Difference in dynamic properties of photoreceptors in a butterfly, Papilio xuthus: possible segregation of motion and color processing. *Journal of Comparative Physiology A*.
- Retinal organization and visual abilities for flower foraging in swallowtail butterflies (*Current Opinion in Insect Science*) — tetrachromatic color vision based on 4 of the 6 spectral receptor classes.

**Frog (*Rana* spp.)**
- Koskelainen, A., Hemilä, S., & Donner, K. (1994). Spectral sensitivities of short- and long-wavelength sensitive cone mechanisms in the frog retina. *Acta Physiologica Scandinavica*, 152, 115–124.
- Denton, E. J., & Wyllie, J. H. (1955). Study of the photosensitive pigments in the pink and green rods of the frog. *Journal of Physiology* (foundational dual rod system description).
- Yovanovich, C. A. M., et al. (2017). The dual rod system of amphibians supports colour discrimination at the absolute visual threshold. *Philosophical Transactions of the Royal Society B*, 372, 20160066.
- Liebman, P. A., & Entine, G. (1968). Visual pigments of frog and tadpole (*Rana pipiens*). *Vision Research*.
- Hárosi, F. I. (1975). Absorption spectra and linear dichroism of some amphibian photoreceptors.
- Muntz, W. R. A. (1966). The photopositive response of the frog (*Rana pipiens*) under photopic and scotopic conditions. *Journal of Experimental Biology*, 45, 101–111.
- A frog's eye view: Foundational revelations and future promises. *Progress in Retinal and Eye Research* / *Developmental Biology* (review covering photopic/scotopic/mesopic colour vision possibilities in anurans).
- Yovanovich, C. A. M., et al. — phototransduction studies on anuran green rods ("super-rods") and their role in scotopic colour vision.
- Diversity and Evolution of Frog Visual Opsins: Spectral Tuning and Adaptation to Distinct Light Environments. *Molecular Biology and Evolution* (opsin λmax ranges across frog species, including SWS1/RH1/SWS2 contributions to the cone and rod systems referenced in §2.3.10).

**Cross-species / general**
- Osorio, D., & Vorobyev, M. (2005). Photoreceptor spectral sensitivities in terrestrial animals: adaptations for luminance and colour vision. *Proceedings of the Royal Society B*, 272, 1745–1752.

### Receptor Noise and the Vorobyev–Osorio RNL Model (§3.3.2, §4)
- Vorobyev, M., & Osorio, D. (1998). Receptor noise as a determinant of colour thresholds. *Proceedings of the Royal Society B*, 265, 351–358.
- Vorobyev, M., Osorio, D., Bennett, A. T. D., Marshall, N. J., & Cuthill, I. C. (1998). Tetrachromacy, oil droplets and bird plumage colours. *Journal of Comparative Physiology A*, 183, 621–633.
- Olsson, P., Lind, O., & Kelber, A. (2015). Bird colour vision: behavioural thresholds reveal receptor noise. *Journal of Experimental Biology* (Weber fraction parameterization conventions).
- Siddiqi, A., Cronin, T. W., Loew, E. R., Vorobyev, M., & Summers, K. (2004). Interspecific and intraspecific views of color signals in the strawberry poison frog, *Dendrobates pumilio* (Weber fraction convention for LWS channel).
- Hart, N. S. (2001). Variations in cone photoreceptor abundance and the visual ecology of birds. *Journal of Comparative Physiology A* (avian cone abundance/density data referenced for pigeon).
- Empirical Imaging knowledge base: "The Receptor Noise Limited Model" and "Cone Ratios & Receptor Noise" (applied-methods reference summarizing RNL formula structure and conventions).

### Natural-Scene-Derived Opponent Contrasts (§4.2.6)
- Buchsbaum, G., & Gottschalk, A. (1983). Trichromacy, opponent colours coding and optimum colour information transmission in the retina. *Proceedings of the Royal Society of London B*, 220(1218), 89–113.
- Ruderman, D. L., Cronin, T. W., & Chiao, C.-C. (1998). Statistics of cone responses to natural images: implications for visual coding. *Journal of the Optical Society of America A*, 15(8), 2036–2045.
- Wachtler, T., Lee, T.-W., & Sejnowski, T. J. (2001). Chromatic structure of natural scenes. *Journal of the Optical Society of America A*, 18(1), 65–77.
- Lee, T.-W., Wachtler, T., & Sejnowski, T. J. (2002). Color opponency is an efficient representation of spectral properties in natural scenes. *Vision Research*, 42(6), 2095–2103.
- Maloney, L. T. (1986). Evaluation of linear models of surface spectral reflectance with small numbers of parameters. *Journal of the Optical Society of America A*, 3(10), 1673–1683 (basis for the parametric smoothness-kernel assumption).

### Hyperspectral/Multispectral Image Formats (§3.4)
- ENVI header file format documentation (NV5 Geospatial / Harris Geospatial Solutions; Opticks ENVI Header Format reference) — the basis for the hand-written header parser in `envi.rs` (§10.3).
- TIFF 6.0 Specification (Adobe) — baseline tag layout (`ImageWidth`/`ImageLength`/`SamplesPerPixel`/`PlanarConfiguration`/etc.) the GeoTIFF reader in `geotiff.rs` depends on.
- `image-rs/tiff` crate documentation — the pure-Rust TIFF decoder GeoTIFF reading is built on (github.com/image-rs/image-tiff).

### Illuminant Generation (§5.4, §5.5)
- Planck's Law (blackbody spectral radiance formula) — standard physics reference (Illuminating Engineering Society definition; standard derivations).
- Fraunhofer lines and telluric O₂ absorption bands (A-band ≈ 759–762 nm, B-band ≈ 686–688 nm) — standard Fraunhofer line tables and compilations.
- Water vapor near-infrared absorption bands — NASA Technical Reports Server: "Comparison of HITRAN Calculated Spectra with Laboratory Measurements of the 820, 940, 1130, and 1370 nm Water Vapor Bands."
- Electromagnetic absorption by water — standard compilation of water absorption band centers (≈940/1100/1450/1950/2500 nm).
- Sodium-vapor lamp spectral characteristics — standard lighting-technology references (RP Photonics; general sodium-vapor lamp literature).
- Mercury-vapor lamp spectral line tables — standard mercury pencil-lamp emission line references (RP Photonics; spectroscopic calibration references).
- White LED (phosphor-converted) spectral power distribution — blue pump-chip + phosphor emission band structure, standard solid-state-lighting technical references.

### Note on Citation Confidence
Several entries above (particularly in Illuminant Generation) were accessed via secondary/aggregated sources rather than original peer-reviewed publications. Where a default model elsewhere in this document is marked as using approximated or lower-confidence data, the corresponding citation should be understood at the same confidence level. Implementers should verify primary sources before relying on exact numerical values for production use.
