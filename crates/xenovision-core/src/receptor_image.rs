//! Receptor-channel image rendering: turning a hyperspectral/multispectral
//! cube (or one selected region of it) into the "primary colors" of a
//! given visual system. Each pixel's spectrum is replaced by one channel
//! per receptor - its activation `Q_i = saturate_i(g_i · ∫ S_i(λ)·I_p(λ) dλ)`,
//! the receptor's sensitivity integrated over the pixel's actual spectrum
//! (with gains from the receptor's illumination operating window and its
//! saturation cap, exactly as the pipeline applies them per stimulus).
//!
//! The unit of rendering is a **colorspace regime** (`RegimeConfig`, see
//! `CurveSet::regime_config`): *[L, C₁..C_(N-1)]* and the adapted
//! activations are properties of one regime's members, so each regime of a
//! multi-regime species (e.g. the frog's scotopic/mesopic/photopic)
//! produces its own channel image. Species with no regimes at all (a
//! Mantis Shrimp, whose receptors never enter any colorspace) are handled
//! by `ReceptorKernel::for_pool`, which renders every receptor in the set
//! individually with its own gain/cap - raw channels only.
//!
//! Two resolvers, one kernel:
//! - `ReceptorKernel::for_regime` - one regime's members; supports the raw
//!   receptor activations, the von Kries-adapted activations, and the
//!   luminance+chroma perceptual channels.
//! - `ReceptorKernel::for_pool` - every receptor in the set's pool,
//!   raw-only (the regime-less / unaffiliated-receptor case).
//!
//! The kernel holds every per-pixel coefficient precomputed (an
//! `N_receptors × N_bands` response matrix, per-receptor gains/caps, and
//! the composed adaptation/opponent matrices), so rendering is a small
//! matrix-vector multiply per pixel - the same shape that makes a wgpu
//! compute path natural (`GpuContext::render_receptors`, with a CPU
//! fallback through `ReceptorKernel::render_with`).
//!
//! Discretization: the per-pixel integral is a trapezoid sum over the
//! cube's own band centers (`w_b · S_i(λ_b) · I_p(λ_b)`), which for a
//! uniform band axis is *identical* to `pipeline::integrate_product`'s
//! trapezoid rule on the same grid. The kernel is the cheap, GPU-friendly
//! formulation of the pipeline's per-stimulus math.

use crate::curve::SpectralCurve;
use crate::curve_set::{CurveSet, RegimeConfig};
use crate::hyperspectral::{HyperspectralCube, Region};
use crate::linalg::Mat;
use crate::pipeline::{self, Pipeline};

/// Hard ceiling on how many receptors one kernel may render, so the GPU
/// shader's fixed-size per-receptor arrays are always large enough. Mantis
/// shrimp is the built-in high-water mark at 12; 64 leaves headroom for
/// any plausible system. The CPU renderer shares the ceiling so a kernel
/// works on either path without a size split.
pub(crate) const MAX_GPU_RECEPTORS: usize = 64;

/// How each pixel's band values are interpreted before receptor
/// integration. Cubes carry no reflectance/radiance metadata of their own,
/// so this is an explicit caller choice (mirroring the manual "extract as
/// an Illumination curve" toggle in the image-import UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// Pixel values are reflectance `ρ(λ)`: each pixel spectrum is
    /// multiplied by the reference luminant before integration, matching
    /// the pipeline's reflectance×illuminant rule. Requires a luminant.
    Reflectance,
    /// Pixel values are radiance as-is. The luminant is optional (used
    /// only as the white point for gating/adaptation when provided).
    Radiance,
}

/// Which pipeline stage defines the output channels of the rendered image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelStage {
    /// `Q_i`, one channel per receptor - the literal "primary colors" for
    /// the visual system. Works with or without a reference light
    /// (without one, gains are flat 1.0).
    RawReceptor,
    /// Von Kries-adapted activations `Q'_i` for a regime's members.
    /// Requires a reference luminant.
    AdaptedReceptor,
    /// `[L, C_1..C_(N-1)]` - luminance and opponent chroma for a regime.
    /// Requires a reference luminant.
    Perceptual,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReceptorKernel {
    stage: ChannelStage,
    regime_name: Option<String>,
    n_receptors: usize,
    n_bands: usize,
    /// Trapezoid weights `Δλ_b` over the band centers, nm.
    band_weights: Vec<f64>,
    /// Row-major `[receptor][band]`: `S_i(λ_b)·Δλ_b`, with the luminant
    /// value `L(λ_b)` folded in for `InputKind::Reflectance`, so a pixel's
    /// raw integral is exactly `Σ_b response[i][b]·pixel[b]`.
    response: Vec<f64>,
    /// Operating-window gain per receptor (all 1.0 when no reference
    /// illuminant), applied *before* the saturation cap.
    gains: Vec<f64>,
    /// Per-receptor saturation caps.
    caps: Vec<Option<f64>>,
    /// Composed von Kries map `Q' = A·Q`, square `N_receptors × N_receptors`.
    adaptation: Option<Mat>,
    /// Perceptual output rows `[L, C_1..]`, each `N_receptors` coefficients
    /// (not necessarily `N` rows - a regime whose literature contrasts
    /// resolve to fewer chroma axes outputs proportionally fewer channels).
    opponent: Option<Vec<Vec<f64>>>,
    /// Output channel names, in output order.
    names: Vec<String>,
    n_out: usize,
    /// `∏ g_i` over the rendered receptors - a regime's overall activity
    /// under the reference light (compare the pipelines' `regime_gain`).
    regime_gain: f64,
    /// `true` if any rendered receptor is substantially operating
    /// (gain > 0.1) - the pipeline's `is_active` semantics.
    is_active: bool,
}

impl ReceptorKernel {
    /// A kernel for one colorspace regime's channels. `config` is the
    /// resolved regime (`CurveSet::regime_config`); `wavelengths_nm` is
    /// the cube band axis the rendered image will be sampled on;
    /// `luminant` is the reference light used for gating/adaptation (and
    /// the reflectance multiplier when `input_kind` is `Reflectance`).
    ///
    /// `AdaptedReceptor`/`Perceptual` and `Reflectance` require a luminant;
    /// `RawReceptor` without one renders ungated (gains 1.0).
    pub fn for_regime(
        config: &RegimeConfig,
        wavelengths_nm: &[f64],
        luminant: Option<&SpectralCurve>,
        input_kind: InputKind,
        stage: ChannelStage,
        step_nm: f64,
    ) -> Result<Self, ReceptorImageError> {
        validate_wavelengths(wavelengths_nm)?;
        let n_bands = wavelengths_nm.len();
        let n_receptors = config.curves.len();
        if n_receptors == 0 {
            return Err(ReceptorImageError::RegimeHasNoMembers);
        }
        if n_receptors > MAX_GPU_RECEPTORS {
            return Err(ReceptorImageError::TooManyReceptors(n_receptors));
        }
        if input_kind == InputKind::Reflectance && luminant.is_none() {
            return Err(ReceptorImageError::MissingLuminant("reflectance rendering"));
        }
        if stage != ChannelStage::RawReceptor && luminant.is_none() {
            return Err(ReceptorImageError::MissingLuminant(match stage {
                ChannelStage::AdaptedReceptor => "adapted-receptor rendering",
                _ => "perceptual rendering",
            }));
        }

        let band_weights = build_band_weights(wavelengths_nm);
        let mut response = vec![0.0; n_receptors * n_bands];
        for (i, curve) in config.curves.iter().enumerate() {
            let row = &mut response[i * n_bands..(i + 1) * n_bands];
            for (b, &wl) in wavelengths_nm.iter().enumerate() {
                let mut v = curve.value_at(wl).unwrap_or(0.0) * band_weights[b];
                if input_kind == InputKind::Reflectance {
                    v *= luminant
                        .expect("reflectance checked above")
                        .value_at(wl)
                        .unwrap_or(0.0);
                }
                row[b] = v;
            }
        }
        let caps: Vec<Option<f64>> = config.curves.iter().map(|c| c.saturation).collect();
        let mut names: Vec<String> = config.curves.iter().map(|c| c.name.clone()).collect();
        let mut n_out = n_receptors;

        let mut gains = vec![1.0; n_receptors];
        let mut regime_gain = 1.0;
        let mut is_active = true;
        let mut adaptation = None;
        let mut opponent = None;
        if let Some(l) = luminant {
            let pipeline = Pipeline::build(config.clone(), l, step_nm);
            gains = pipeline.response_gains().to_vec();
            regime_gain = pipeline.regime_gain();
            is_active = pipeline.is_active();
            match stage {
                ChannelStage::RawReceptor => {}
                ChannelStage::AdaptedReceptor => {
                    adaptation = Some(pipeline.adaptation_transform());
                }
                ChannelStage::Perceptual => {
                    adaptation = Some(pipeline.adaptation_transform());
                    let mut rows = vec![pipeline.luminance_weights().to_vec()];
                    for axis in pipeline.chroma_axes() {
                        rows.push(axis.clone());
                    }
                    n_out = rows.len();
                    names = std::iter::once("L".to_string())
                        .chain((1..n_out).map(|i| format!("C{i}")))
                        .collect();
                    opponent = Some(rows);
                }
            }
        }

        Ok(ReceptorKernel {
            stage,
            regime_name: Some(config.name.clone()),
            n_receptors,
            n_bands,
            band_weights,
            response,
            gains,
            caps,
            adaptation,
            opponent,
            names,
            n_out,
            regime_gain,
            is_active,
        })
    }

    /// A raw-receptor kernel over **every** receptor in the set's pool, in
    /// pool order - the mode for species with no regimes at all, and for
    /// viewing unaffiliated receptors (in no regime) alongside the rest.
    /// Gating honors the set-level illumination-window defaults via
    /// `CurveSet::receptor_gains`. Raw-only by construction: adaptation and
    /// opponent channels are properties of a regime this pool view has no
    /// access to.
    pub fn for_pool(
        set: &CurveSet,
        wavelengths_nm: &[f64],
        luminant: Option<&SpectralCurve>,
        input_kind: InputKind,
        step_nm: f64,
    ) -> Result<Self, ReceptorImageError> {
        validate_wavelengths(wavelengths_nm)?;
        let n_bands = wavelengths_nm.len();
        let n_receptors = set.curves.len();
        if n_receptors == 0 {
            return Err(ReceptorImageError::NoReceptors);
        }
        if n_receptors > MAX_GPU_RECEPTORS {
            return Err(ReceptorImageError::TooManyReceptors(n_receptors));
        }
        if input_kind == InputKind::Reflectance && luminant.is_none() {
            return Err(ReceptorImageError::MissingLuminant("reflectance rendering"));
        }

        let band_weights = build_band_weights(wavelengths_nm);
        let mut response = vec![0.0; n_receptors * n_bands];
        for (i, curve) in set.curves.iter().enumerate() {
            let row = &mut response[i * n_bands..(i + 1) * n_bands];
            for (b, &wl) in wavelengths_nm.iter().enumerate() {
                let mut v = curve.value_at(wl).unwrap_or(0.0) * band_weights[b];
                if input_kind == InputKind::Reflectance {
                    v *= luminant
                        .expect("reflectance checked above")
                        .value_at(wl)
                        .unwrap_or(0.0);
                }
                row[b] = v;
            }
        }
        let caps: Vec<Option<f64>> = set.curves.iter().map(|c| c.saturation).collect();
        let gains: Vec<f64> = match luminant {
            Some(l) => {
                let seen = pipeline::seen_irradiances_for(&set.curves, l, step_nm);
                set.receptor_gains(&seen)
            }
            None => vec![1.0; n_receptors],
        };
        let regime_gain = gains.iter().product();
        let is_active = gains.iter().any(|g| *g > 0.1);
        let names: Vec<String> = set.curves.iter().map(|c| c.name.clone()).collect();

        Ok(ReceptorKernel {
            stage: ChannelStage::RawReceptor,
            regime_name: None,
            n_receptors,
            n_bands,
            band_weights,
            response,
            gains,
            caps,
            adaptation: None,
            opponent: None,
            names,
            n_out: n_receptors,
            regime_gain,
            is_active,
        })
    }

    /// One kernel per regime of `set`, in `set.regimes` order - the
    /// multi-regime orchestration entry point (a species' distinct
    /// colorspaces each get their own channel image). An empty `set.regimes`
    /// yields an empty result; for a regime-less species use
    /// [`ReceptorKernel::for_pool`] instead.
    pub fn kernels_for_all_regimes(
        set: &CurveSet,
        wavelengths_nm: &[f64],
        luminant: Option<&SpectralCurve>,
        input_kind: InputKind,
        stage: ChannelStage,
        step_nm: f64,
    ) -> Result<Vec<ReceptorKernel>, ReceptorImageError> {
        set.regimes
            .iter()
            .map(|r| {
                let config = set.regime_config(r);
                ReceptorKernel::for_regime(
                    &config,
                    wavelengths_nm,
                    luminant,
                    input_kind,
                    stage,
                    step_nm,
                )
            })
            .collect()
    }

    pub fn stage(&self) -> ChannelStage {
        self.stage
    }

    /// The regime this kernel renders, or `None` for a pool kernel
    /// ([`ReceptorKernel::for_pool`]).
    pub fn regime_name(&self) -> Option<&str> {
        self.regime_name.as_deref()
    }

    /// Output channel count (receptors for raw/adapted stages; the
    /// luminance + chroma row count for the perceptual stage).
    pub fn n_channels(&self) -> usize {
        self.n_out
    }

    /// Output channel names, in output order.
    pub fn channel_names(&self) -> &[String] {
        &self.names
    }

    /// The rendered receptors' overall activity under the reference light -
    /// `∏ g_i`. Near 1 only where every receptor's own window contains its
    /// seen irradiance; a dim-only regime is ~0 in bright light. Used to
    /// pick which regime's image to show (mirrors `Pipeline::regime_gain`).
    pub fn regime_gain(&self) -> f64 {
        self.regime_gain
    }

    /// `true` if at least one rendered receptor is substantially
    /// operating (gain above 0.1) under the reference light (mirrors
    /// `Pipeline::is_active`).
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// Renders the cube (or one region of it) on the CPU, in parallel over
    /// pixel rows.
    pub fn render(
        &self,
        cube: &HyperspectralCube,
        region: Option<&Region>,
    ) -> Result<ReceptorImage, ReceptorImageError> {
        if cube.bands != self.n_bands {
            return Err(ReceptorImageError::BandCountMismatch(
                self.n_bands,
                cube.bands,
            ));
        }
        let (x0, x1, y0, y1, width, height) =
            image_bounds(cube, region).ok_or(ReceptorImageError::EmptyRegion)?;
        let n_pixels = width * height;
        let n_out = self.n_out;
        let mask = region_mask(region, x0, x1, y0, y1, width);

        let mut data = vec![0.0f32; n_out * n_pixels];
        let y_range: Vec<usize> = (y0..y1).collect();
        let chunk = y_range.len().div_ceil(worker_count(y_range.len()));
        let results = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for rows in y_range.chunks(chunk) {
                let rows = rows.to_vec();
                let kernel = self;
                handles.push(scope.spawn(move || {
                    let start_row = rows[0];
                    let block_rows = rows.len();
                    let block = block_rows * width;
                    let mut buf = vec![0.0f32; n_out * block];
                    let mut raw = vec![0.0f64; kernel.n_receptors];
                    let mut q = vec![0.0f64; kernel.n_receptors];
                    for (ri, &row) in rows.iter().enumerate() {
                        for col in x0..x1 {
                            let p = ri * width + (col - x0);
                            raw.fill(0.0);
                            for b in 0..kernel.n_bands {
                                let v = cube.pixel(row, col, b) as f64;
                                if v == 0.0 {
                                    continue;
                                }
                                let resp = &kernel.response;
                                for r in 0..kernel.n_receptors {
                                    raw[r] += resp[r * kernel.n_bands + b] * v;
                                }
                            }
                            for r in 0..kernel.n_receptors {
                                let gated = raw[r] * kernel.gains[r];
                                q[r] = match kernel.caps[r] {
                                    Some(cap) => gated.min(cap),
                                    None => gated,
                                };
                            }
                            if let Some(a) = &kernel.adaptation {
                                q = a.mul_vec(&q);
                            }
                            match &kernel.opponent {
                                Some(rows) => {
                                    for (c, row_coeffs) in rows.iter().enumerate() {
                                        let mut acc = 0.0;
                                        for (k, &w) in row_coeffs.iter().enumerate() {
                                            acc += w * q[k];
                                        }
                                        buf[c * block + p] = acc as f32;
                                    }
                                }
                                None => {
                                    for r in 0..kernel.n_receptors {
                                        buf[r * block + p] = q[r] as f32;
                                    }
                                }
                            }
                        }
                    }
                    (start_row, buf)
                }));
            }
            handles
                .into_iter()
                .map(|h| h.join().expect("receptor render thread panicked"))
                .collect::<Vec<_>>()
        });

        // Scatter each row block into the channel-major output. Row blocks
        // are disjoint, so the per-channel `copy_from_slice` writes are
        // disjoint too.
        for (start_row, buf) in results {
            let p0 = (start_row - y0) * width;
            let block = buf.len() / n_out;
            for c in 0..n_out {
                let base = c * n_pixels + p0;
                let src = &buf[c * block..(c + 1) * block];
                data[base..base + block].copy_from_slice(src);
            }
        }

        let range = compute_range(&data, n_out, n_pixels, mask.as_deref());
        Ok(ReceptorImage {
            width,
            height,
            channels: self.names.clone(),
            stage: self.stage,
            regime_name: self.regime_name.clone(),
            data,
            range,
            mask,
        })
    }

    /// Renders on the GPU via `gpu`'s shared wgpu context. `None` on any
    /// GPU-side failure (no adapter, upload/readback problems, dimensions
    /// outside the kernel's support) - callers fall back to
    /// [`ReceptorKernel::render`].
    pub fn render_gpu(
        &self,
        gpu: &crate::gpu::GpuContext,
        cube: &HyperspectralCube,
        region: Option<&Region>,
    ) -> Option<ReceptorImage> {
        if cube.bands != self.n_bands {
            return None;
        }
        let (x0, x1, y0, y1, width, height) = image_bounds(cube, region)?;
        let n_pixels = width * height;
        let mask = region_mask(region, x0, x1, y0, y1, width);

        // Whole-image/no-region storage is already the cube's band-major
        // layout, so upload it without a gather; a region needs its bbox
        // collected once into a contiguous band-major buffer.
        let cube_region: Vec<f32> = if region.is_none() {
            cube.data().to_vec()
        } else {
            let mut buf = vec![0.0f32; self.n_bands * n_pixels];
            for b in 0..self.n_bands {
                for row in y0..y1 {
                    let base = b * n_pixels + (row - y0) * width;
                    for col in x0..x1 {
                        buf[base + (col - x0)] = cube.pixel(row, col, b);
                    }
                }
            }
            buf
        };

        let data = gpu.render_receptors(self, &cube_region, n_pixels)?;
        let range = compute_range(&data, self.n_out, n_pixels, mask.as_deref());
        Some(ReceptorImage {
            width,
            height,
            channels: self.names.clone(),
            stage: self.stage,
            regime_name: self.regime_name.clone(),
            data,
            range,
            mask,
        })
    }

    /// Renders via the GPU when available (falls back silently to the CPU
    /// on any GPU failure) - the entry point callers that don't need to
    /// own the fallback policy can use directly.
    pub fn render_with(
        &self,
        gpu: Option<&crate::gpu::GpuContext>,
        cube: &HyperspectralCube,
        region: Option<&Region>,
    ) -> Result<ReceptorImage, ReceptorImageError> {
        if let Some(g) = gpu {
            if let Some(img) = self.render_gpu(g, cube, region) {
                return Ok(img);
            }
        }
        self.render(cube, region)
    }

    // --- internals shared with the GPU renderer (same crate) ---

    pub(crate) fn n_out(&self) -> usize {
        self.n_out
    }

    pub(crate) fn n_receptors(&self) -> usize {
        self.n_receptors
    }

    pub(crate) fn n_bands(&self) -> usize {
        self.n_bands
    }

    pub(crate) fn response(&self) -> &[f64] {
        &self.response
    }

    pub(crate) fn gains(&self) -> &[f64] {
        &self.gains
    }

    pub(crate) fn caps(&self) -> &[Option<f64>] {
        &self.caps
    }

    pub(crate) fn adaptation_mat(&self) -> Option<&Mat> {
        self.adaptation.as_ref()
    }

    pub(crate) fn opponent_rows(&self) -> Option<&[Vec<f64>]> {
        self.opponent.as_deref()
    }

    /// Matrix `A` such that `Q' = A·Q` (adapted activations), when the
    /// stage uses one.
    pub fn adaptation_transform(&self) -> Option<&Mat> {
        self.adaptation.as_ref()
    }
}

/// The rendered image: one channel per receptor (or perceptual channel),
/// in the selected region's pixel grid. Channel-major layout
/// (`data[c*width*height + row*width + col]`) is the shape a GPU round-trip
/// produces, and the one a future display mapping / export path can walk
/// directly.
#[derive(Debug, Clone)]
pub struct ReceptorImage {
    pub width: usize,
    pub height: usize,
    /// Channel names, in output order.
    pub channels: Vec<String>,
    pub stage: ChannelStage,
    /// The regime this image renders, or `None` for a pool render.
    pub regime_name: Option<String>,
    /// Channel-major as documented on the struct.
    data: Vec<f32>,
    /// Per-channel `(min, max)` over masked-in pixels (or all pixels when
    /// there is no mask), for display scaling.
    range: Vec<(f32, f32)>,
    /// `None` = whole-image render (every pixel valid); `Some` = region
    /// membership over the bbox.
    mask: Option<Vec<bool>>,
}

impl ReceptorImage {
    pub fn n_pixels(&self) -> usize {
        self.width * self.height
    }

    /// One channel's pixel values (channel-major), or `None` for an out-of-
    /// range channel index.
    pub fn channel(&self, c: usize) -> Option<&[f32]> {
        if c >= self.channels.len() {
            return None;
        }
        let n_pixels = self.n_pixels();
        Some(&self.data[c * n_pixels..(c + 1) * n_pixels])
    }

    /// Iterates one pixel's values across every channel, in channel order.
    pub fn pixel(&self, x: usize, y: usize) -> PixelChannels<'_> {
        PixelChannels {
            image: self,
            x,
            y,
            c: 0,
        }
    }

    /// Region membership mask, when the render was a region (rectangle or
    /// polygon) - `None` only for a whole-image render. Matches
    /// `hyperspectral::region_pixel_indices`: a bbox pixel is masked in
    /// iff its center falls inside the region.
    pub fn mask(&self) -> Option<&[bool]> {
        self.mask.as_deref()
    }

    /// Per-channel `(min, max)` over masked-in pixels.
    pub fn range(&self) -> &[(f32, f32)] {
        &self.range
    }

    /// One channel's pixels normalized to 0..1 against its computed range,
    /// respecting the region mask (masked-out pixels are 0.0). `None` for
    /// an out-of-range channel. A denormalized channel (flat image, zero
    /// span) yields all zeros.
    pub fn normalized_channel(&self, c: usize) -> Option<Vec<f32>> {
        let slice = self.channel(c)?;
        let (lo, hi) = self.range.get(c).copied()?;
        let span = (hi - lo).max(f32::MIN_POSITIVE);
        let mask = self.mask();
        Some(
            slice
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    if mask.is_none_or(|m| m[i]) {
                        ((v - lo) / span).clamp(0.0, 1.0)
                    } else {
                        0.0
                    }
                })
                .collect(),
        )
    }
}

/// Iterator over one pixel's channel values (channel-major storage).
pub struct PixelChannels<'a> {
    image: &'a ReceptorImage,
    x: usize,
    y: usize,
    c: usize,
}

impl Iterator for PixelChannels<'_> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.c >= self.image.channels.len() {
            return None;
        }
        let v =
            self.image.data[self.c * self.image.n_pixels() + self.y * self.image.width + self.x];
        self.c += 1;
        Some(v)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReceptorImageError {
    #[error("the cube has no wavelength axis assigned")]
    MissingWavelengths,
    #[error("wavelengths must be strictly increasing, but {0} >= {1}")]
    NotIncreasing(f64, f64),
    #[error("the regime has no member receptors")]
    RegimeHasNoMembers,
    #[error("the set has no receptor curves (empty pool)")]
    NoReceptors,
    #[error("a reference luminant is required for {0}")]
    MissingLuminant(&'static str),
    #[error("the region contains no pixels")]
    EmptyRegion,
    #[error("the kernel was built for {0} bands but the cube has {1}")]
    BandCountMismatch(usize, usize),
    #[error("this kernel renders {0} receptors, over the {MAX_GPU_RECEPTORS}-receptor rendering ceiling")]
    TooManyReceptors(usize),
}

fn worker_count(items: usize) -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, items.max(1))
}

fn validate_wavelengths(wavelengths: &[f64]) -> Result<(), ReceptorImageError> {
    if wavelengths.is_empty() {
        return Err(ReceptorImageError::MissingWavelengths);
    }
    for pair in wavelengths.windows(2) {
        if pair[1] <= pair[0] {
            return Err(ReceptorImageError::NotIncreasing(pair[0], pair[1]));
        }
    }
    Ok(())
}

/// Trapezoid weights over band centers: half-widths at the edges, full
/// spacing in between - identical to `pipeline::integrate_product`'s
/// trapezoid rule when the band axis is uniform at `step_nm` and spans the
/// same domain.
fn build_band_weights(wavelengths: &[f64]) -> Vec<f64> {
    let n = wavelengths.len();
    let mut w = vec![0.0; n];
    if n == 1 {
        w[0] = 1.0;
    } else {
        w[0] = 0.5 * (wavelengths[1] - wavelengths[0]);
        for b in 1..n - 1 {
            w[b] = 0.5 * (wavelengths[b + 1] - wavelengths[b - 1]);
        }
        w[n - 1] = 0.5 * (wavelengths[n - 1] - wavelengths[n - 2]);
    }
    w
}

/// The pixel range covered by a region (or the whole cube), in the same
/// pixel-center convention as `hyperspectral::region_pixel_indices`. Returns
/// `(x0, x1, y0, y1, width, height)` or `None` if nothing falls in bounds.
fn image_bounds(
    cube: &HyperspectralCube,
    region: Option<&Region>,
) -> Option<(usize, usize, usize, usize, usize, usize)> {
    let (x_min, y_min, x_max, y_max) = match region {
        Some(r) => r.bounds(),
        None => (0.0, 0.0, cube.samples as f64, cube.lines as f64),
    };
    let clamp = |v: f64, hi: usize| (v.max(0.0) as usize).min(hi);
    let x0 = clamp(x_min - 0.5, cube.samples);
    let x1 = clamp(x_max + 0.5, cube.samples);
    let y0 = clamp(y_min - 0.5, cube.lines);
    let y1 = clamp(y_max + 0.5, cube.lines);
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    let (width, height) = (x1 - x0, y1 - y0);
    Some((x0, x1, y0, y1, width, height))
}

/// The membership mask over a region's bbox: every region render carries
/// one (matching `hyperspectral::region_pixel_indices`' pixel-center
/// test), because a bbox built from `bounds() ± 0.5` can include partial
/// edge columns. `None` only for a whole-image render, whose bbox is the
/// whole grid.
fn region_mask(
    region: Option<&Region>,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    width: usize,
) -> Option<Vec<bool>> {
    let region = region?;
    let n_pixels = (y1 - y0) * width;
    let mut m = vec![false; n_pixels];
    for row in y0..y1 {
        for col in x0..x1 {
            m[(row - y0) * width + (col - x0)] =
                region.contains(col as f64 + 0.5, row as f64 + 0.5);
        }
    }
    Some(m)
}

/// Per-channel `(min, max)` over masked-in pixels; a channel with no
/// masked-in pixels (or an all-masked-out region) reports `(0.0, 0.0)`.
fn compute_range(
    data: &[f32],
    n_channels: usize,
    n_pixels: usize,
    mask: Option<&[bool]>,
) -> Vec<(f32, f32)> {
    (0..n_channels)
        .map(|c| {
            let slice = &data[c * n_pixels..(c + 1) * n_pixels];
            let mut lo = f64::INFINITY;
            let mut hi = f64::NEG_INFINITY;
            for (i, &v) in slice.iter().enumerate() {
                if mask.is_none_or(|m| m[i]) {
                    let f = v as f64;
                    if f < lo {
                        lo = f;
                    }
                    if f > hi {
                        hi = f;
                    }
                }
            }
            if lo.is_infinite() {
                (0.0, 0.0)
            } else {
                (lo as f32, hi as f32)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::{CurveType, QuantityKind};
    use crate::pipeline::{raw_activations_with_gains, Pipeline};

    const AXIS: [f64; 7] = [400.0, 450.0, 500.0, 550.0, 600.0, 650.0, 700.0];
    const STEP_NM: f64 = 50.0;

    fn gaussian(name: &str, mu: f64, sigma: f64) -> SpectralCurve {
        let points: Vec<(f64, f64)> = (400..=700)
            .map(|wl| {
                let wl = wl as f64;
                let d = (wl - mu) / sigma;
                (wl, (-0.5 * d * d).exp())
            })
            .collect();
        SpectralCurve::new(name, CurveType::Sensitivity).with_points(points)
    }

    fn flat_light() -> SpectralCurve {
        SpectralCurve::new("Flat", CurveType::Illumination)
            .with_quantity(QuantityKind::Radiance {
                unit: "W/m²/nm".to_string(),
            })
            .with_points(
                AXIS.iter()
                    .map(|&wl| (wl, 1.0))
                    .collect::<Vec<(f64, f64)>>(),
            )
    }

    fn trichromat_set() -> CurveSet {
        let mut set = CurveSet::new("Test trichromat");
        for c in [
            gaussian("S", 440.0, 35.0),
            gaussian("M", 530.0, 40.0),
            gaussian("L", 570.0, 45.0),
        ] {
            set.curves.push(c);
        }
        set.regimes
            .push(crate::curve_set::Regime::new("Photopic", vec![0, 1, 2]));
        set
    }

    fn test_cube(samples: usize, lines: usize, data: Vec<f32>) -> HyperspectralCube {
        HyperspectralCube::from_band_major(
            samples,
            lines,
            AXIS.len(),
            Some(AXIS.to_vec()),
            data,
            "test",
        )
    }

    fn filled_cube(samples: usize, lines: usize) -> HyperspectralCube {
        let mut data = vec![0.0f32; samples * lines * AXIS.len()];
        for b in 0..AXIS.len() {
            for row in 0..lines {
                for col in 0..samples {
                    data[b * lines * samples + row * samples + col] =
                        (b * 3 + row * 2 + col) as f32 * 0.125;
                }
            }
        }
        test_cube(samples, lines, data)
    }

    fn pixel_curve(cube: &HyperspectralCube, row: usize, col: usize) -> SpectralCurve {
        let points = AXIS
            .iter()
            .enumerate()
            .map(|(b, &wl)| (wl, cube.pixel(row, col, b) as f64))
            .collect();
        SpectralCurve::new("pixel", CurveType::Illumination)
            .with_quantity(QuantityKind::Radiance {
                unit: "W/m²/nm".to_string(),
            })
            .with_points(points)
    }

    #[test]
    fn band_sum_matches_trapezoid_integration() {
        // A flat receptor S=1 over the full axis: the kernel's band sum
        // must equal pipeline::integrate_product's trapezoid rule.
        let mut set = trichromat_set();
        set.curves[0] = SpectralCurve::new("FlatS", CurveType::Sensitivity).with_points(
            AXIS.iter()
                .map(|&wl| (wl, 1.0))
                .collect::<Vec<(f64, f64)>>(),
        );
        let config = set.regime_config(&set.regimes[0]);

        // One pixel, known values.
        let cube = test_cube(1, 1, vec![2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        let kernel = ReceptorKernel::for_regime(
            &config,
            &AXIS,
            None,
            InputKind::Radiance,
            ChannelStage::RawReceptor,
            STEP_NM,
        )
        .unwrap();
        let img = kernel.render(&cube, None).unwrap();
        // Trapezoid over 7 bands, spacing 50: 25*v0 + 50*(v1..v5) + 25*v6.
        let expected = 25.0 * 2.0 + 50.0 * (3.0 + 4.0 + 5.0 + 6.0 + 7.0) + 25.0 * 8.0;
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 1);
        let got = img.channel(0).unwrap()[0] as f64;
        assert!(
            (got - expected).abs() < 1e-6,
            "band sum {got} != trapezoid {expected}"
        );

        // And it matches integrate_product's dense rule directly (same
        // grid, same weights).
        let pixel = pixel_curve(&cube, 0, 0);
        let integral = crate::pipeline::integrate_product(&config.curves[0], &pixel, STEP_NM);
        assert!(
            (got - integral).abs() < 1e-6,
            "band sum {got} != integrate_product {integral}"
        );
    }

    #[test]
    fn raw_activations_match_pipeline_per_pixel() {
        let set = trichromat_set();
        let config = set.regime_config(&set.regimes[0]);
        let light = flat_light();
        let pipeline = Pipeline::build(config.clone(), &light, STEP_NM);
        let cube = filled_cube(10, 8);

        let kernel_raw = ReceptorKernel::for_regime(
            &config,
            &AXIS,
            Some(&light),
            InputKind::Radiance,
            ChannelStage::RawReceptor,
            STEP_NM,
        )
        .unwrap();
        let img = kernel_raw.render(&cube, None).unwrap();

        for row in 0..8 {
            for col in 0..10 {
                let pcurve = pixel_curve(&cube, row, col);
                let expected = raw_activations_with_gains(
                    &config.curves,
                    pipeline.response_gains(),
                    &pcurve,
                    STEP_NM,
                );
                let pix: Vec<f32> = img.pixel(col, row).collect();
                for (c, &e) in expected.iter().enumerate() {
                    let d = (pix[c] as f64 - e).abs();
                    assert!(
                        d < 1e-6 * (1.0 + e.abs()),
                        "raw [{row},{col}][{c}]: got {}, expected {e}",
                        pix[c]
                    );
                }
            }
        }
    }

    #[test]
    fn adapted_and_perceptual_match_pipeline_per_pixel() {
        let set = trichromat_set();
        let config = set.regime_config(&set.regimes[0]);
        let light = flat_light();
        let pipeline = Pipeline::build(config.clone(), &light, STEP_NM);
        let cube = filled_cube(6, 5);

        let kernel_adapted = ReceptorKernel::for_regime(
            &config,
            &AXIS,
            Some(&light),
            InputKind::Radiance,
            ChannelStage::AdaptedReceptor,
            STEP_NM,
        )
        .unwrap();
        let img_adapted = kernel_adapted.render(&cube, None).unwrap();
        assert_eq!(img_adapted.channels, ["S", "M", "L"]);

        let kernel_perc = ReceptorKernel::for_regime(
            &config,
            &AXIS,
            Some(&light),
            InputKind::Radiance,
            ChannelStage::Perceptual,
            STEP_NM,
        )
        .unwrap();
        let img_perc = kernel_perc.render(&cube, None).unwrap();
        // L + two chroma axes for a trichromat.
        assert_eq!(img_perc.channels, ["L", "C1", "C2"]);
        assert_eq!(kernel_perc.n_channels(), 3);

        for row in 0..5 {
            for col in 0..6 {
                let pcurve = pixel_curve(&cube, row, col);
                let adapted = pipeline.adapted_activations(&pcurve);
                let pix_adapted: Vec<f32> = img_adapted.pixel(col, row).collect();
                for (c, &e) in adapted.iter().enumerate() {
                    let d = (pix_adapted[c] as f64 - e).abs();
                    assert!(
                        d < 1e-6 * (1.0 + e.abs()),
                        "adapted [{row},{col}][{c}]: got {}, expected {e}",
                        pix_adapted[c]
                    );
                }

                let coords = pipeline.coordinates(&pcurve);
                let expected = [coords.luminance, coords.chroma[0], coords.chroma[1]];
                let pix_perc: Vec<f32> = img_perc.pixel(col, row).collect();
                for (c, &e) in expected.iter().enumerate() {
                    let d = (pix_perc[c] as f64 - e).abs();
                    assert!(
                        d < 1e-6 * (1.0 + e.abs()),
                        "perceptual [{row},{col}][{c}]: got {}, expected {e}",
                        pix_perc[c]
                    );
                }
            }
        }
    }

    #[test]
    fn reflectance_equals_radiance_of_premultiplied_pixels() {
        let set = trichromat_set();
        let config = set.regime_config(&set.regimes[0]);
        // Non-flat light so the luminant factor visibly matters.
        let light = SpectralCurve::new("Sloped", CurveType::Illumination)
            .with_quantity(QuantityKind::Radiance {
                unit: "W/m²/nm".to_string(),
            })
            .with_points(
                AXIS.iter()
                    .enumerate()
                    .map(|(i, &wl)| (wl, 1.0 + 0.25 * i as f64))
                    .collect(),
            );
        let cube = filled_cube(4, 4);

        let reflect = ReceptorKernel::for_regime(
            &config,
            &AXIS,
            Some(&light),
            InputKind::Reflectance,
            ChannelStage::RawReceptor,
            STEP_NM,
        )
        .unwrap();
        let img_reflect = reflect.render(&cube, None).unwrap();

        // Same pixel values, but pre-multiplied by the luminant, fed to a
        // radiance-stage kernel: must give identical channels.
        let mut premult = cube.data().to_vec();
        for b in 0..AXIS.len() {
            let l = light.value_at(AXIS[b]).unwrap();
            for p in 0..cube.samples * cube.lines {
                premult[b * cube.samples * cube.lines + p] *= l as f32;
            }
        }
        let cube2 = test_cube(4, 4, premult);
        let radiance = ReceptorKernel::for_regime(
            &config,
            &AXIS,
            Some(&light),
            InputKind::Radiance,
            ChannelStage::RawReceptor,
            STEP_NM,
        )
        .unwrap();
        let img_radiance = radiance.render(&cube2, None).unwrap();

        let n_pixels = img_reflect.n_pixels();
        for c in 0..img_reflect.channels.len() {
            let a = img_reflect.channel(c).unwrap();
            let b = img_radiance.channel(c).unwrap();
            for i in 0..n_pixels {
                assert!(
                    (a[i] - b[i]).abs() <= 1e-4,
                    "channel {c} pixel {i}: reflectance {} != radiance {}",
                    a[i],
                    b[i]
                );
            }
        }
    }

    #[test]
    fn saturation_cap_applies_after_gain() {
        let mut set = trichromat_set();
        set.curves[0].saturation = Some(2.0);
        let config = set.regime_config(&set.regimes[0]);
        let cube = test_cube(1, 1, vec![1.0; AXIS.len()]);
        // No luminant: gains flat 1.0, so the raw integral is the full
        // trapezoid over the pixel (25 + 5*50 + 25 = 300).
        let kernel = ReceptorKernel::for_regime(
            &config,
            &AXIS,
            None,
            InputKind::Radiance,
            ChannelStage::RawReceptor,
            STEP_NM,
        )
        .unwrap();
        let img = kernel.render(&cube, None).unwrap();
        // S has the cap: 300 -> 2.0. M/L uncapped: their (roughly) unit
        // integrals stay well above the cap so we only assert the cap side.
        assert_eq!(img.channel(0).unwrap()[0], 2.0);
    }

    #[test]
    fn pool_kernel_honors_windows_and_unaffiliated_receptors() {
        let mut set = CurveSet::new("Frog-pool");
        set.curves
            .push(gaussian("Rod", 505.0, 40.0).with_illumination_window(Some(1.0), Some(10.0)));
        set.curves.push(
            gaussian("M-cone", 530.0, 40.0).with_illumination_window(Some(30.0), Some(1000.0)),
        );
        // A third receptor in no regime at all - still rendered by for_pool.
        set.curves.push(
            gaussian("L-cone", 570.0, 45.0).with_illumination_window(Some(30.0), Some(1000.0)),
        );
        set.regimes
            .push(crate::curve_set::Regime::new("Photopic", vec![1, 2]));

        let light = flat_light();
        let kernel =
            ReceptorKernel::for_pool(&set, &AXIS, Some(&light), InputKind::Radiance, STEP_NM)
                .unwrap();
        assert_eq!(kernel.channel_names(), ["Rod", "M-cone", "L-cone"]);
        assert!(kernel.regime_name().is_none());
        // Three pool channels; Rod (window far below the ~100 W/m² it sees)
        // gated essentially off, both cones on.
        assert_eq!(kernel.n_channels(), 3);
        assert!(kernel.is_active());

        let cube = filled_cube(3, 3);
        let img = kernel.render(&cube, None).unwrap();
        let rod = img.channel(0).unwrap();
        let cone_m = img.channel(1).unwrap();
        // Rod's gain is tiny, cones' gains are near 1, at every pixel.
        for i in 0..img.n_pixels() {
            assert!(
                rod[i].abs() < cone_m[i],
                "rod activation {} should be far below cone {}",
                rod[i],
                cone_m[i]
            );
        }
    }

    #[test]
    fn multi_regime_activity_tracks_light_level() {
        let mut set = CurveSet::new("Frog");
        set.curves
            .push(gaussian("Rod", 505.0, 40.0).with_illumination_window(Some(1.0), Some(10.0)));
        set.curves
            .push(gaussian("Cone", 530.0, 40.0).with_illumination_window(Some(30.0), Some(1000.0)));
        set.regimes
            .push(crate::curve_set::Regime::new("Scotopic", vec![0]));
        set.regimes
            .push(crate::curve_set::Regime::new("Photopic", vec![1]));

        let light = flat_light();
        let kernels = ReceptorKernel::kernels_for_all_regimes(
            &set,
            &AXIS,
            Some(&light),
            InputKind::Radiance,
            ChannelStage::RawReceptor,
            STEP_NM,
        )
        .unwrap();
        assert_eq!(kernels.len(), 2);
        assert_eq!(kernels[0].regime_name(), Some("Scotopic"));
        assert_eq!(kernels[1].regime_name(), Some("Photopic"));

        // In bright light the scotopic regime is off; photopic is on.
        assert!(
            !kernels[0].is_active(),
            "scotopic should be inactive in bright light"
        );
        assert!(
            kernels[0].regime_gain() < 0.05,
            "scotopic regime_gain {}",
            kernels[0].regime_gain()
        );
        assert!(
            kernels[1].is_active(),
            "photopic should be active in bright light"
        );
        assert!(
            kernels[1].regime_gain() > 0.8,
            "photopic regime_gain {}",
            kernels[1].regime_gain()
        );
    }

    #[test]
    fn regime_less_set_errors_on_forced_regime_and_has_empty_regimes() {
        let mut set = CurveSet::new("Mantis");
        set.curves.push(gaussian("UV", 360.0, 30.0));
        set.curves.push(gaussian("R", 570.0, 45.0));
        // No regimes declared at all.
        let light = flat_light();
        let kernels = ReceptorKernel::kernels_for_all_regimes(
            &set,
            &AXIS,
            Some(&light),
            InputKind::Radiance,
            ChannelStage::RawReceptor,
            STEP_NM,
        )
        .unwrap();
        assert!(kernels.is_empty());

        // The zero-member degenerate config can't render a regime stage.
        let degenerate = set.default_regime_config();
        let err = ReceptorKernel::for_regime(
            &degenerate,
            &AXIS,
            Some(&light),
            InputKind::Radiance,
            ChannelStage::Perceptual,
            STEP_NM,
        )
        .unwrap_err();
        assert!(matches!(err, ReceptorImageError::RegimeHasNoMembers));

        // Pool rendering works fine for the regime-less set.
        let pool =
            ReceptorKernel::for_pool(&set, &AXIS, Some(&light), InputKind::Radiance, STEP_NM)
                .unwrap();
        assert_eq!(pool.channel_names(), ["UV", "R"]);
    }

    #[test]
    fn polygon_region_produces_mask_and_ranges_over_interior() {
        let cube = filled_cube(8, 8);
        // Axis-aligned square in the middle of the 8x8 grid.
        let region = Region::Polygon(vec![(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0)]);
        let set = trichromat_set();
        let config = set.regime_config(&set.regimes[0]);
        let kernel = ReceptorKernel::for_regime(
            &config,
            &AXIS,
            None,
            InputKind::Radiance,
            ChannelStage::RawReceptor,
            STEP_NM,
        )
        .unwrap();
        let img = kernel.render(&cube, Some(&region)).unwrap();

        // bbox cols/rows 0..3 (clamp(1-0.5)=0 .. clamp(3+0.5)=3), so 3x3;
        // centers (1.5,2.5 in both axes) land inside the [1,3) square -> the
        // 4 interior marks.
        assert_eq!((img.width, img.height), (3, 3));
        let mask = img.mask().expect("region renders masked");
        assert_eq!(mask.len(), 9);
        assert_eq!(mask.iter().filter(|b| **b).count(), 4);

        // The masked-out pixels must be excluded from the channel ranges.
        let range = img.range();
        for (c, &(lo, hi)) in range.iter().enumerate() {
            for (i, &inside) in mask.iter().enumerate() {
                let v = img.channel(c).unwrap()[i];
                if inside {
                    assert!(
                        v >= lo - 1e-6 && v <= hi + 1e-6,
                        "channel {c}: interior pixel {v} outside range {lo}..{hi}"
                    );
                }
            }
        }
        assert!(img.range()[0].0 < img.range()[0].1);
    }

    #[test]
    fn rectangle_regions_are_center_masked_like_the_existing_selection() {
        let cube = filled_cube(8, 8);
        let region = Region::Rectangle {
            x0: 2.0,
            y0: 2.0,
            x1: 5.0,
            y1: 5.0,
        };
        let set = trichromat_set();
        let config = set.regime_config(&set.regimes[0]);
        let kernel = ReceptorKernel::for_regime(
            &config,
            &AXIS,
            None,
            InputKind::Radiance,
            ChannelStage::RawReceptor,
            STEP_NM,
        )
        .unwrap();
        let img = kernel.render(&cube, Some(&region)).unwrap();
        // bbox cols/rows 1..5 (clamp(2-0.5)=1 .. clamp(5+0.5)=5), centers
        // inside [2,5) -> the 3x3 set {2,3,4}x{2,3,4} - the same pixels
        // `region_pixel_indices` would extract for this region.
        assert_eq!((img.width, img.height), (4, 4));
        let mask = img.mask().expect("region renders carry a mask");
        assert_eq!(mask.len(), 16);
        assert_eq!(mask.iter().filter(|b| **b).count(), 9);
        for row in 0..4 {
            for col in 0..4 {
                // bbox pixel (col, row) is cube pixel (col+x0, row+y0)
                // with x0 = clamp(2-0.5) = 1.
                let cx = col as f64 + 1.0 + 0.5;
                let cy = row as f64 + 1.0 + 0.5;
                let center_in = (2.0..5.0).contains(&cx) && (2.0..5.0).contains(&cy);
                assert_eq!(
                    mask[row * 4 + col],
                    center_in,
                    "mask bit ({col},{row}) should be {center_in}"
                );
            }
        }
    }

    #[test]
    fn empty_region_and_band_mismatch_error() {
        let cube = filled_cube(4, 4);
        let set = trichromat_set();
        let config = set.regime_config(&set.regimes[0]);
        let kernel = ReceptorKernel::for_regime(
            &config,
            &AXIS,
            None,
            InputKind::Radiance,
            ChannelStage::RawReceptor,
            STEP_NM,
        )
        .unwrap();

        let off = Region::Rectangle {
            x0: 50.0,
            y0: 50.0,
            x1: 60.0,
            y1: 60.0,
        };
        assert!(matches!(
            kernel.render(&cube, Some(&off)),
            Err(ReceptorImageError::EmptyRegion)
        ));

        let wrong = HyperspectralCube::from_band_major(
            2,
            2,
            5,
            Some(vec![400.0, 450.0, 500.0, 550.0, 600.0]),
            vec![0.0; 20],
            "wrong",
        );
        assert!(matches!(
            kernel.render(&wrong, None),
            Err(ReceptorImageError::BandCountMismatch(7, 5))
        ));
    }

    #[test]
    fn normalized_channel_respects_mask_and_range() {
        let cube = test_cube(1, 1, vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let set = trichromat_set();
        let config = set.regime_config(&set.regimes[0]);
        // All receptors over a pixel of zeros..6: the smallest band-sum is
        // receptor S (peaks at 440 -> 0), the largest is L (570 -> 6-ish).
        let kernel = ReceptorKernel::for_regime(
            &config,
            &AXIS,
            None,
            InputKind::Radiance,
            ChannelStage::RawReceptor,
            STEP_NM,
        )
        .unwrap();
        let img = kernel.render(&cube, None).unwrap();
        for c in 0..img.channels.len() {
            let norm = img.normalized_channel(c).unwrap();
            assert!(
                norm.iter().all(|&v| (0.0..=1.0).contains(&v)),
                "normalized channel {c} out of [0,1]"
            );
        }
        assert!(img.normalized_channel(99).is_none());
        assert_eq!(img.channel(99), None);
    }

    #[test]
    fn gpu_matches_cpu_render() {
        let Some(ctx) = crate::gpu::try_init() else {
            eprintln!("no GPU adapter available - skipping");
            return;
        };
        let set = trichromat_set();
        let config = set.regime_config(&set.regimes[0]);
        let light = flat_light();
        let cube = filled_cube(64, 48);

        let kernel = ReceptorKernel::for_regime(
            &config,
            &AXIS,
            Some(&light),
            InputKind::Radiance,
            ChannelStage::AdaptedReceptor,
            STEP_NM,
        )
        .unwrap();
        let cpu = kernel.render(&cube, None).unwrap();
        let gpu = kernel.render_gpu(&ctx, &cube, None).expect("GPU render");

        assert_eq!(cpu.n_pixels(), gpu.n_pixels());
        for c in 0..cpu.channels.len() {
            let a = cpu.channel(c).unwrap();
            let b = gpu.channel(c).unwrap();
            let denom = a
                .iter()
                .map(|v| f64::from(v.abs()))
                .fold(0.0f64, f64::max)
                .max(1.0);
            for i in 0..a.len() {
                let d = (a[i] as f64 - b[i] as f64).abs();
                assert!(
                    d < 2e-3 * denom,
                    "channel {c} pixel {i}: cpu {} gpu {} (d={d})",
                    a[i],
                    b[i]
                );
            }
        }

        // And the fallback entry point returns the GPU image.
        let via_with = kernel.render_with(Some(&ctx), &cube, None).unwrap();
        assert_eq!(via_with.channel(0).unwrap(), gpu.channel(0).unwrap());
    }

    #[test]
    fn gpu_dispatch_spans_two_dimensions() {
        let Some(ctx) = crate::gpu::try_init() else {
            eprintln!("no GPU adapter available - skipping");
            return;
        };
        // Pixels > 65535 workgroups * 64 invocations = 4,194,240: the
        // dispatch must split across the y dimension or wgpu panics.
        let (samples, lines) = (2049usize, 2048usize); // 4,196,352 pixels
        let n_pixels = samples * lines;
        let mut data = vec![0.0f32; n_pixels * AXIS.len()];
        for (i, v) in data.iter_mut().enumerate() {
            *v = ((i % 97) * 2) as f32 * 0.01;
        }
        let cube = test_cube(samples, lines, data);

        let set = trichromat_set();
        let config = set.regime_config(&set.regimes[0]);
        let kernel = ReceptorKernel::for_regime(
            &config,
            &AXIS,
            None,
            InputKind::Radiance,
            ChannelStage::RawReceptor,
            STEP_NM,
        )
        .unwrap();

        let gpu = kernel.render_gpu(&ctx, &cube, None).expect("GPU rendered");
        assert_eq!(gpu.n_pixels(), n_pixels);
        let cpu = kernel.render(&cube, None).unwrap();
        for c in 0..cpu.channels.len() {
            let a = cpu.channel(c).unwrap();
            let b = gpu.channel(c).unwrap();
            let denom = a
                .iter()
                .map(|v| f64::from(v.abs()))
                .fold(0.0f64, f64::max)
                .max(1.0);
            for i in 0..a.len() {
                let d = (a[i] as f64 - b[i] as f64).abs();
                assert!(
                    d < 2e-3 * denom,
                    "channel {c} pixel {i}: cpu {} gpu {} (d={d})",
                    a[i],
                    b[i]
                );
            }
        }
    }
}
