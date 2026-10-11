//! Pooled wgpu compute for the core library's two batched workloads.
//!
//! **1. The receptor-image renderer (active).** `render_receptors` renders
//! every pixel of a hyperspectral cube (or region) through a
//! [`ReceptorKernel`](crate::receptor_image::ReceptorKernel) in one
//! dispatch: per-pixel band-summed receptor activation, gating, saturation
//! cap, the composed von Kries adaptation map, and the regime's opponent
//! output rows - exactly the CPU path in
//! [`ReceptorKernel::render`](crate::receptor_image::ReceptorKernel::render),
//! batched as pixel per invocation. Dispatches larger than 65535 workgroups
//! are split across the dispatch's y dimension (see `render_receptors`).
//!
//! **2. The adaptation-objective batch (currently unused).** `adaptation`
//! used to route its finite-difference evaluations here for N > 16, but
//! its exact-gradient CPU method is now faster at every size measured, so
//! nothing calls that path. It's kept as a second working wgpu setup
//! sharing the same pooling scheme as the receptor path.
//!
//! Measured with `bench_cpu_vs_gpu_overlap_sum` (`cargo test -p
//! xenovision-core --release -- --ignored --nocapture` to reproduce):
//! one finite-difference gradient step - `1 + 2*(N^2 - N)` objective
//! evaluations of one species' curves - computed as a CPU loop over every
//! candidate versus one batched GPU dispatch pair, against the analytic-
//! gradient CPU step that replaced it. Receptors are Gaussians evenly
//! spaced over a 451-sample (300-750nm @ 1nm) grid. Measured on an Intel
//! HD Graphics 4600 (`backend=Gl`, driver 4.3.0 / 20.19.15.4835 - no
//! Vulkan, no hardware D3D12 on this box):
//!
//! | N  | batch | CPU [ms] | GPU [ms] | CPU/GPU | exact-grad CPU [ms] |
//! |----|-------|----------|----------|---------|---------------------|
//! | 3  | 13    | 0.070    | 1.456    | 0.05    | 0.0027              |
//! | 5  | 41    | 0.586    | 1.606    | 0.36    | 0.0069              |
//! | 8  | 113   | 4.139    | 3.318    | 1.25    | 0.0149              |
//! | 12 | 265   | 22.431   | 14.523   | 1.54    | 0.0333              |
//! | 16 | 481   | 72.936   | 46.512   | 1.57    | 0.0921              |
//!
//! The GPU's per-call buffers are pooled and reused (see `Pool` below), so
//! once a size has been seen the only per-call costs are the input upload
//! and the dispatch/readback round-trip. That reclaimed the allocation
//! overhead which previously dominated and let the GPU overtake the CPU
//! loop around N=8 and reach ~1.5x by N=16. At small N the fixed
//! round-trip still favors the CPU, which leads by 3-20x there. The
//! analytic-gradient step, which replaces the entire finite-difference
//! batch with one `O(n^3)` pass, remains far faster than either. These
//! numbers are hardware-specific (a discrete GPU would push the crossover
//! further left), but the exact-gradient CPU method keeps the GPU off the
//! critical path either way.
//!
//! Known limitation of the adaptation-objective pass: it is one 1-D
//! dispatch, so batches larger than 65535 workgroups (reached around N≈20
//! for the adaptation batch) exceed wgpu's per-dimension limit and panic
//! instead of returning `None`. The receptor renderer does not share this -
//! it splits such dispatches across the y dimension (`render_receptors`).
//!
//! `try_init` returns `None` if no compute-capable adapter is available
//! or setup fails, so callers can always fall back to the CPU.
//!
//! Two compute passes mirror `adaptation.rs`'s `objective()` exactly
//! (same candidate-matrix construction, same pairwise-overlap-sum
//! formula), batched over every finite-difference-perturbed matrix a
//! gradient step needs (`1 + 2*(n*n-n)` of them) in one dispatch pair
//! instead of one CPU call per perturbation:
//! - `transform_batch`: for each (batch entry, output curve, grid
//!   point), computes the candidate-matrix-transformed curve value.
//! - `overlap_batch`: for each (batch entry, receptor pair), dot-
//!   products the two transformed curves and squares the result -
//!   summing these per batch entry on the CPU (trivial, `O(n^2)` tiny
//!   values) reproduces `objective()`'s `overlap_sum` term exactly.
//!
//! Verified correct (not just "compiles") against a hand-calculated
//! case and against the CPU path directly - see `tests` below.

use std::sync::Mutex;

use crate::receptor_image::{ReceptorKernel, MAX_GPU_RECEPTORS};

const SHADER_SRC: &str = r#"
struct Params {
    n: u32,
    grid_len: u32,
    batch_size: u32,
    num_pairs: u32,
    step_nm: f32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> curves: array<f32>;
@group(0) @binding(2) var<storage, read> offdiag_batch: array<f32>;
@group(0) @binding(3) var<storage, read_write> transformed: array<f32>;

fn offdiag_index(i: u32, j: u32, n: u32) -> u32 {
    // Matches adaptation.rs's mat_from_offdiag: row-major, diagonal
    // entries skipped (n-1 stored off-diagonal entries per row).
    if (j < i) {
        return i * (n - 1u) + j;
    } else {
        return i * (n - 1u) + (j - 1u);
    }
}

@compute @workgroup_size(64)
fn transform_batch(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    let total = params.batch_size * params.n * params.grid_len;
    if (idx >= total) {
        return;
    }
    let g = idx % params.grid_len;
    let i = (idx / params.grid_len) % params.n;
    let b = idx / (params.grid_len * params.n);

    var sum: f32 = 0.0;
    for (var k: u32 = 0u; k < params.n; k = k + 1u) {
        var m_ik: f32 = 1.0;
        if (k != i) {
            let off_base = b * (params.n * params.n - params.n);
            m_ik = offdiag_batch[off_base + offdiag_index(i, k, params.n)];
        }
        sum = sum + m_ik * curves[k * params.grid_len + g];
    }
    transformed[idx] = sum;
}

@group(0) @binding(0) var<uniform> params2: Params;
@group(0) @binding(1) var<storage, read> transformed2: array<f32>;
@group(0) @binding(2) var<storage, read_write> pair_overlap: array<f32>;

@compute @workgroup_size(64)
fn overlap_batch(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    let total = params2.batch_size * params2.num_pairs;
    if (idx >= total) {
        return;
    }
    let pair = idx % params2.num_pairs;
    let b = idx / params2.num_pairs;

    var i: u32 = 0u;
    var remaining: u32 = pair;
    loop {
        let row_len = params2.n - 1u - i;
        if (remaining < row_len) {
            break;
        }
        remaining = remaining - row_len;
        i = i + 1u;
    }
    let j = i + 1u + remaining;

    var dot: f32 = 0.0;
    for (var g: u32 = 0u; g < params2.grid_len; g = g + 1u) {
        dot = dot + transformed2[(b * params2.n + i) * params2.grid_len + g]
                  * transformed2[(b * params2.n + j) * params2.grid_len + g];
    }
    let ov = dot * params2.step_nm;
    pair_overlap[idx] = ov * ov;
}
"#;

/// The receptor-image renderer's compute shader: one invocation renders one
/// pixel, mirroring `ReceptorKernel::render`'s CPU per-pixel math exactly
/// (band-summed raw activations, gating, saturation cap, the composed von
/// Kries adaptation map, and the regime's opponent rows). All non-pixel
/// inputs (`response`, `gains`, `caps`, `adaptation`, `opponent`) are
/// kernel-level constants, so one pipeline serves every stage and regime -
/// each dispatch just swaps which buffers are bound.
///
/// The pixel index is `gid.y * PIXELS_PER_SLICE + gid.x` (a "slice" is
/// `64 * 65535 = 4,194,240` pixels - the largest one 1-D dispatch can
/// cover), so `render_receptors`' 2-D dispatch keeps every supported image
/// inside wgpu's 65535-workgroup-per-dimension limit.
const RECEPTOR_SHADER_SRC: &str = r#"
const PIXELS_PER_SLICE: u32 = 64u * 65535u;

struct RParams {
    n_bands: u32,
    n_receptors: u32,
    n_out: u32,
    n_pixels: u32,
    has_adaptation: u32,
    has_opponent: u32,
    _pad0: u32,
    _pad1: u32,
};

@group(0) @binding(0) var<uniform> params: RParams;
@group(0) @binding(1) var<storage, read> cube: array<f32>;
@group(0) @binding(2) var<storage, read> response: array<f32>;
@group(0) @binding(3) var<storage, read> gains: array<f32>;
// Caps as (present, value) pairs per receptor, so a genuine cap of 0.0 is
// distinguishable from "no cap".
@group(0) @binding(4) var<storage, read> caps: array<f32>;
@group(0) @binding(5) var<storage, read> adaptation: array<f32>;
@group(0) @binding(6) var<storage, read> opponent: array<f32>;
@group(0) @binding(7) var<storage, read_write> rout: array<f32>;

// Per-invocation scratch (private storage class is per-invocation), sized
// for the kernel's receptor ceiling.
var<private> raw: array<f32, 64>;
var<private> q: array<f32, 64>;

@compute @workgroup_size(64)
fn receptor_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p: u32 = gid.y * PIXELS_PER_SLICE + gid.x;
    if (p >= params.n_pixels) {
        return;
    }
    let n_rep: u32 = min(params.n_receptors, 64u);

    // Raw activation, gating, saturation cap - `Q_i` per receptor.
    for (var r: u32 = 0u; r < n_rep; r = r + 1u) {
        var acc: f32 = 0.0;
        let resp_base: u32 = r * params.n_bands;
        for (var b: u32 = 0u; b < params.n_bands; b = b + 1u) {
            acc = acc + response[resp_base + b] * cube[b * params.n_pixels + p];
        }
        var gated: f32 = acc * gains[r];
        if (caps[r * 2u] > 0.5 && gated > caps[r * 2u + 1u]) {
            gated = caps[r * 2u + 1u];
        }
        raw[r] = gated;
        q[r] = gated;
    }

    // Composed von Kries map `Q' = A·Q` (row-major N x N), when the
    // kernel carries one.
    if (params.has_adaptation == 1u) {
        for (var i: u32 = 0u; i < n_rep; i = i + 1u) {
            var acc: f32 = 0.0;
            for (var j: u32 = 0u; j < n_rep; j = j + 1u) {
                acc = acc + adaptation[i * n_rep + j] * raw[j];
            }
            q[i] = acc;
        }
    }

    // Opponent rows `[L, C1..]`, or the plain (adapted) receptor channels.
    if (params.has_opponent == 1u) {
        for (var c: u32 = 0u; c < params.n_out; c = c + 1u) {
            var acc: f32 = 0.0;
            for (var i: u32 = 0u; i < n_rep; i = i + 1u) {
                acc = acc + opponent[c * n_rep + i] * q[i];
            }
            rout[c * params.n_pixels + p] = acc;
        }
    } else {
        for (var c: u32 = 0u; c < n_rep; c = c + 1u) {
            rout[c * params.n_pixels + p] = q[c];
        }
    }
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    n: u32,
    grid_len: u32,
    batch_size: u32,
    num_pairs: u32,
    step_nm: f32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

/// Uniform payload for `RECEPTOR_SHADER_SRC`'s `receptor_main`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RParams {
    n_bands: u32,
    n_receptors: u32,
    n_out: u32,
    n_pixels: u32,
    has_adaptation: u32,
    has_opponent: u32,
    _pad0: u32,
    _pad1: u32,
}

fn alloc_buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    })
}

/// Rounds a required byte size up to a power of two (never below 4, since
/// a zero-sized buffer is invalid), so capacities grow geometrically and a
/// later call needing only slightly more reuses the buffer instead of
/// reallocating. Binding a whole-but-larger buffer is safe here: both
/// shaders bounds-check every access against `batch_size*n*grid_len` /
/// `batch_size*num_pairs`, so the unused tail is never read or written.
fn pooled_capacity(needed: u64) -> u64 {
    needed.max(4).next_power_of_two()
}

/// GPU buffers reused across `overlap_sum_batch` calls, so a steady stream
/// of same-sized batches (the finite-difference case this module exists
/// for) does no allocation at all. Only the per-call input *contents* are
/// re-uploaded; capacities persist and grow. Grouped behind one `Mutex` in
/// [`GpuContext`]; everything here is `wgpu`-handle-only and cheap to hold.
struct Pool {
    params: wgpu::Buffer,
    curves: wgpu::Buffer,
    curves_cap: u64,
    offdiag: wgpu::Buffer,
    offdiag_cap: u64,
    transformed: wgpu::Buffer,
    transformed_cap: u64,
    pair_overlap: wgpu::Buffer,
    pair_overlap_cap: u64,
    readback: wgpu::Buffer,
    readback_cap: u64,
    transform_bg: wgpu::BindGroup,
    overlap_bg: wgpu::BindGroup,
}

/// Builds the two bind groups for a set of pool buffers. Both are rebuilt
/// whenever any buffer they reference is (re)allocated, since a bind group
/// captures the buffer handle it was built from.
fn make_bind_groups(
    ctx: &GpuContext,
    params: &wgpu::Buffer,
    curves: &wgpu::Buffer,
    offdiag: &wgpu::Buffer,
    transformed: &wgpu::Buffer,
    pair_overlap: &wgpu::Buffer,
) -> (wgpu::BindGroup, wgpu::BindGroup) {
    let transform_bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pool_transform_bg"),
        layout: &ctx.transform_pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: curves.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: offdiag.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: transformed.as_entire_binding(),
            },
        ],
    });
    let overlap_bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pool_overlap_bg"),
        layout: &ctx.overlap_pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: transformed.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: pair_overlap.as_entire_binding(),
            },
        ],
    });
    (transform_bg, overlap_bg)
}

impl Pool {
    fn new(
        ctx: &GpuContext,
        curves: u64,
        offdiag: u64,
        transformed: u64,
        pair_overlap: u64,
    ) -> Pool {
        let params = alloc_buffer(
            &ctx.device,
            "pool_params",
            std::mem::size_of::<Params>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let curves_cap = pooled_capacity(curves);
        let curves_buf = alloc_buffer(
            &ctx.device,
            "pool_curves",
            curves_cap,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let offdiag_cap = pooled_capacity(offdiag);
        let offdiag_buf = alloc_buffer(
            &ctx.device,
            "pool_offdiag",
            offdiag_cap,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let transformed_cap = pooled_capacity(transformed);
        let transformed_buf = alloc_buffer(
            &ctx.device,
            "pool_transformed",
            transformed_cap,
            wgpu::BufferUsages::STORAGE,
        );
        let pair_overlap_cap = pooled_capacity(pair_overlap);
        let pair_overlap_buf = alloc_buffer(
            &ctx.device,
            "pool_pair_overlap",
            pair_overlap_cap,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let readback_cap = pooled_capacity(pair_overlap);
        let readback_buf = alloc_buffer(
            &ctx.device,
            "pool_readback",
            readback_cap,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );

        let (transform_bg, overlap_bg) = make_bind_groups(
            ctx,
            &params,
            &curves_buf,
            &offdiag_buf,
            &transformed_buf,
            &pair_overlap_buf,
        );
        Pool {
            params,
            curves: curves_buf,
            curves_cap,
            offdiag: offdiag_buf,
            offdiag_cap,
            transformed: transformed_buf,
            transformed_cap,
            pair_overlap: pair_overlap_buf,
            pair_overlap_cap,
            readback: readback_buf,
            readback_cap,
            transform_bg,
            overlap_bg,
        }
    }
}

/// GPU buffers reused across `render_receptors` calls (same pooling scheme
/// as [`Pool`]). The cube contents still change per call, but input size
/// and the kernel-level buffers (`response`/`gains`/`caps`/`adaptation`/
/// `opponent`) change only with kernel shape, so a steady stream of
/// same-shape renders reuses every buffer. `caps` needs `2*N` values (a
/// (present, value) pair per receptor), hence its own field.
struct ReceptorPool {
    params: wgpu::Buffer,
    cube: wgpu::Buffer,
    cube_cap: u64,
    response: wgpu::Buffer,
    response_cap: u64,
    gains: wgpu::Buffer,
    gains_cap: u64,
    caps: wgpu::Buffer,
    caps_cap: u64,
    adaptation: wgpu::Buffer,
    adaptation_cap: u64,
    opponent: wgpu::Buffer,
    opponent_cap: u64,
    rout: wgpu::Buffer,
    rout_cap: u64,
    readback: wgpu::Buffer,
    readback_cap: u64,
    bg: wgpu::BindGroup,
}

#[allow(clippy::too_many_arguments)]
fn make_receptor_bind_group(
    ctx: &GpuContext,
    params: &wgpu::Buffer,
    cube: &wgpu::Buffer,
    response: &wgpu::Buffer,
    gains: &wgpu::Buffer,
    caps: &wgpu::Buffer,
    adaptation: &wgpu::Buffer,
    opponent: &wgpu::Buffer,
    rout: &wgpu::Buffer,
) -> wgpu::BindGroup {
    ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("receptor_bg"),
        layout: &ctx.receptor_pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: cube.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: response.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: gains.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: caps.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: adaptation.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: opponent.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: rout.as_entire_binding(),
            },
        ],
    })
}

impl ReceptorPool {
    /// An all-minimum-size pool (`adaptation`/`opponent` get a 4-byte
    /// placeholder when their kernel lacks them; the shader never reads a
    /// disabled buffer, and any missing binding would trip validation, so a
    /// dummy is required either way).
    #[allow(clippy::too_many_arguments)]
    fn new(
        ctx: &GpuContext,
        cube: u64,
        response: u64,
        gains: u64,
        caps: u64,
        adaptation: u64,
        opponent: u64,
        rout: u64,
    ) -> ReceptorPool {
        let params = alloc_buffer(
            &ctx.device,
            "receptor_params",
            std::mem::size_of::<RParams>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let cube_cap = pooled_capacity(cube);
        let cube_buf = alloc_buffer(
            &ctx.device,
            "receptor_cube",
            cube_cap,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let response_cap = pooled_capacity(response);
        let response_buf = alloc_buffer(
            &ctx.device,
            "receptor_response",
            response_cap,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let gains_cap = pooled_capacity(gains);
        let gains_buf = alloc_buffer(
            &ctx.device,
            "receptor_gains",
            gains_cap,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let caps_cap = pooled_capacity(caps);
        let caps_buf = alloc_buffer(
            &ctx.device,
            "receptor_caps",
            caps_cap,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let adaptation_cap = pooled_capacity(adaptation);
        let adaptation_buf = alloc_buffer(
            &ctx.device,
            "receptor_adaptation",
            adaptation_cap,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let opponent_cap = pooled_capacity(opponent);
        let opponent_buf = alloc_buffer(
            &ctx.device,
            "receptor_opponent",
            opponent_cap,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let rout_cap = pooled_capacity(rout);
        let rout_buf = alloc_buffer(
            &ctx.device,
            "receptor_rout",
            rout_cap,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let readback_cap = pooled_capacity(rout);
        let readback_buf = alloc_buffer(
            &ctx.device,
            "receptor_readback",
            readback_cap,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );

        let bg = make_receptor_bind_group(
            ctx,
            &params,
            &cube_buf,
            &response_buf,
            &gains_buf,
            &caps_buf,
            &adaptation_buf,
            &opponent_buf,
            &rout_buf,
        );
        ReceptorPool {
            params,
            cube: cube_buf,
            cube_cap,
            response: response_buf,
            response_cap,
            gains: gains_buf,
            gains_cap,
            caps: caps_buf,
            caps_cap,
            adaptation: adaptation_buf,
            adaptation_cap,
            opponent: opponent_buf,
            opponent_cap,
            rout: rout_buf,
            rout_cap,
            readback: readback_buf,
            readback_cap,
            bg,
        }
    }
}

pub struct GpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    transform_pipeline: wgpu::ComputePipeline,
    overlap_pipeline: wgpu::ComputePipeline,
    /// The receptor-image renderer's pipeline (one shader for every stage/
    /// regime - only the bind buffers change per call).
    receptor_pipeline: wgpu::ComputePipeline,
    /// Lazily built on the first `overlap_sum_batch`, then reused. `Mutex`
    /// (not `RefCell`) so `GpuContext` stays `Sync` for callers that might
    /// share it; calls are serialized through it either way, which is what
    /// a single command stream wants anyway.
    pool: Mutex<Option<Pool>>,
    /// Lazily built on the first `render_receptors`, then reused (same
    /// `Mutex`-shared scheme as `pool`).
    receptor_pool: Mutex<Option<ReceptorPool>>,
}

/// Tries to find a compute-capable GPU adapter and build the two
/// pipelines above. `None` on any failure (no adapter, device
/// creation failed, shader failed to compile/validate) - every failure
/// mode is treated the same way: GPU acceleration simply isn't used.
pub fn try_init() -> Option<GpuContext> {
    let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
    instance_desc.backends = wgpu::Backends::all();
    let instance = wgpu::Instance::new(instance_desc);

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .ok()?;

    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("adaptation_matrix"),
        source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
    });

    let transform_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("transform_batch"),
        layout: None,
        module: &module,
        entry_point: Some("transform_batch"),
        compilation_options: Default::default(),
        cache: None,
    });
    let overlap_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("overlap_batch"),
        layout: None,
        module: &module,
        entry_point: Some("overlap_batch"),
        compilation_options: Default::default(),
        cache: None,
    });

    let receptor_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("receptor_render"),
        source: wgpu::ShaderSource::Wgsl(RECEPTOR_SHADER_SRC.into()),
    });
    let receptor_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("receptor_render"),
        layout: None,
        module: &receptor_module,
        entry_point: Some("receptor_main"),
        compilation_options: Default::default(),
        cache: None,
    });

    Some(GpuContext {
        device,
        queue,
        transform_pipeline,
        overlap_pipeline,
        receptor_pipeline,
        pool: Mutex::new(None),
        receptor_pool: Mutex::new(None),
    })
}

impl GpuContext {
    /// For each entry in `offdiag_batch` (`n*n - n` off-diagonal
    /// candidate-matrix parameters each), returns the sum-of-squared-
    /// pairwise-overlaps term - exactly `adaptation::objective`'s
    /// `overlap_sum`, computed for every batch entry in one GPU
    /// dispatch pair instead of one CPU call per entry. The caller
    /// still adds the (trivial) regularization term itself. `None` on
    /// any GPU-side failure (buffer mapping, device poll, etc.) - the
    /// caller falls back to a CPU-computed objective for that call
    /// rather than treating a transient GPU hiccup as a zero result.
    pub fn overlap_sum_batch(
        &self,
        sampled: &[Vec<f64>],
        n: usize,
        step_nm: f64,
        offdiag_batch: &[Vec<f64>],
    ) -> Option<Vec<f64>> {
        let grid_len = sampled.first().map(|c| c.len()).unwrap_or(0);
        let batch_size = offdiag_batch.len();
        let num_pairs = n * (n.saturating_sub(1)) / 2;
        if n == 0 || grid_len == 0 || batch_size == 0 || num_pairs == 0 {
            return Some(vec![0.0; batch_size]);
        }

        let curves_f32: Vec<f32> = sampled
            .iter()
            .flat_map(|c| c.iter().map(|&v| v as f32))
            .collect();
        let offdiag_f32: Vec<f32> = offdiag_batch
            .iter()
            .flat_map(|row| row.iter().map(|&v| v as f32))
            .collect();

        let params = Params {
            n: n as u32,
            grid_len: grid_len as u32,
            batch_size: batch_size as u32,
            num_pairs: num_pairs as u32,
            step_nm: step_nm as f32,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
        };

        let curves_bytes = curves_f32.len() as u64 * 4;
        let offdiag_bytes = offdiag_f32.len() as u64 * 4;
        let transformed_bytes = (batch_size * n * grid_len) as u64 * 4;
        let pair_overlap_bytes = (batch_size * num_pairs) as u64 * 4;

        // Serialize GPU calls and reuse the pooled buffers. Reallocation
        // happens only when a buffer is too small for this call, so a
        // steady stream of same-sized batches allocates nothing after the
        // first.
        let mut guard = self.pool.lock().unwrap();
        if guard.is_none() {
            *guard = Some(Pool::new(
                self,
                curves_bytes,
                offdiag_bytes,
                transformed_bytes,
                pair_overlap_bytes,
            ));
        }
        let pool = guard.as_mut().unwrap();

        let mut rebuild = false;
        if pool.curves_cap < curves_bytes {
            pool.curves_cap = pooled_capacity(curves_bytes);
            pool.curves = alloc_buffer(
                &self.device,
                "pool_curves",
                pool.curves_cap,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            rebuild = true;
        }
        if pool.offdiag_cap < offdiag_bytes {
            pool.offdiag_cap = pooled_capacity(offdiag_bytes);
            pool.offdiag = alloc_buffer(
                &self.device,
                "pool_offdiag",
                pool.offdiag_cap,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            rebuild = true;
        }
        if pool.transformed_cap < transformed_bytes {
            pool.transformed_cap = pooled_capacity(transformed_bytes);
            pool.transformed = alloc_buffer(
                &self.device,
                "pool_transformed",
                pool.transformed_cap,
                wgpu::BufferUsages::STORAGE,
            );
            rebuild = true;
        }
        if pool.pair_overlap_cap < pair_overlap_bytes {
            pool.pair_overlap_cap = pooled_capacity(pair_overlap_bytes);
            pool.pair_overlap = alloc_buffer(
                &self.device,
                "pool_pair_overlap",
                pool.pair_overlap_cap,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            );
            rebuild = true;
        }
        if pool.readback_cap < pair_overlap_bytes {
            pool.readback_cap = pooled_capacity(pair_overlap_bytes);
            pool.readback = alloc_buffer(
                &self.device,
                "pool_readback",
                pool.readback_cap,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            );
            // readback is in no bind group, so it never forces a rebuild.
        }
        if rebuild {
            let (transform_bg, overlap_bg) = make_bind_groups(
                self,
                &pool.params,
                &pool.curves,
                &pool.offdiag,
                &pool.transformed,
                &pool.pair_overlap,
            );
            pool.transform_bg = transform_bg;
            pool.overlap_bg = overlap_bg;
        }

        // Only the per-call input contents are (re)uploaded.
        self.queue
            .write_buffer(&pool.params, 0, bytemuck::bytes_of(&params));
        self.queue
            .write_buffer(&pool.curves, 0, bytemuck::cast_slice(&curves_f32));
        self.queue
            .write_buffer(&pool.offdiag, 0, bytemuck::cast_slice(&offdiag_f32));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&self.transform_pipeline);
            pass.set_bind_group(0, &pool.transform_bg, &[]);
            let total1 = (batch_size * n * grid_len) as u32;
            pass.dispatch_workgroups(total1.div_ceil(64), 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&self.overlap_pipeline);
            pass.set_bind_group(0, &pool.overlap_bg, &[]);
            let total2 = (batch_size * num_pairs) as u32;
            pass.dispatch_workgroups(total2.div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&pool.pair_overlap, 0, &pool.readback, 0, pair_overlap_bytes);
        self.queue.submit(Some(encoder.finish()));

        let slice = pool.readback.slice(..pair_overlap_bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        if rx.recv().ok()?.is_err() {
            return None;
        }
        let out = match slice.get_mapped_range() {
            Ok(mapped) => {
                let pair_overlaps: &[f32] = bytemuck::cast_slice(&mapped);
                (0..batch_size)
                    .map(|b| {
                        pair_overlaps[b * num_pairs..(b + 1) * num_pairs]
                            .iter()
                            .map(|&v| v as f64)
                            .sum::<f64>()
                    })
                    .collect::<Vec<f64>>()
            }
            Err(_) => return None,
        };
        // Unmap so the pooled readback buffer is reusable next call.
        pool.readback.unmap();
        Some(out)
    }

    /// Renders a whole pixel block through `kernel` in one compute
    /// dispatch: the channel-major result of
    /// [`ReceptorKernel::render`](crate::receptor_image::ReceptorKernel::render)
    /// (`n_channels * n_pixels` f32 values, channel-major). `cube_region`
    /// is the block's band-major pixel spectra (`n_bands * n_pixels` f32
    /// values) - for a whole-image render this is exactly
    /// `HyperspectralCube::data()`, which is why the CPU gather is skipped
    /// in that case.
    ///
    /// Dispatches are split across the y dimension so images larger than
    /// `64 * 65535 = 4,194,240` pixels stay inside wgpu's 65535-workgroup
    /// per-dimension limit (a 2049×2048 image - 4,196,352 pixels - is the
    /// smallest such case). `None` on any GPU-side failure; callers fall
    /// back to the CPU.
    pub fn render_receptors(
        &self,
        kernel: &ReceptorKernel,
        cube_region: &[f32],
        n_pixels: usize,
    ) -> Option<Vec<f32>> {
        let n_bands = kernel.n_bands();
        let n_receptors = kernel.n_receptors();
        let n_out = kernel.n_out();
        if n_receptors == 0 || n_pixels == 0 || n_bands == 0 {
            return Some(Vec::new());
        }
        if n_receptors > MAX_GPU_RECEPTORS {
            return None;
        }

        // Kernel-level constants, flattened to f32 payloads. `caps` is one
        // (present, value) pair per receptor so a genuine cap of 0.0
        // renders correctly (None -> [0.0, 0.0]).
        let response_f32: Vec<f32> = kernel.response().iter().map(|&v| v as f32).collect();
        let gains_f32: Vec<f32> = kernel.gains().iter().map(|&v| v as f32).collect();
        let caps_f32: Vec<f32> = kernel
            .caps()
            .iter()
            .flat_map(|c| match c {
                Some(v) => vec![1.0, *v as f32],
                None => vec![0.0, 0.0],
            })
            .collect();
        let adaptation_f32: Vec<f32> = kernel
            .adaptation_mat()
            .map(|m| m.data().iter().map(|&v| v as f32).collect())
            .unwrap_or_default();
        let opponent_f32: Vec<f32> = kernel
            .opponent_rows()
            .map(|rows| rows.iter().flatten().map(|&v| v as f32).collect())
            .unwrap_or_default();

        let params = RParams {
            n_bands: n_bands as u32,
            n_receptors: n_receptors as u32,
            n_out: n_out as u32,
            n_pixels: n_pixels as u32,
            has_adaptation: u32::from(kernel.adaptation_mat().is_some()),
            has_opponent: u32::from(kernel.opponent_rows().is_some()),
            _pad0: 0,
            _pad1: 0,
        };

        let bytes = |len: usize| len as u64 * 4;
        let cube_bytes = bytes(cube_region.len());
        let response_bytes = bytes(response_f32.len());
        let gains_bytes = bytes(gains_f32.len());
        let caps_bytes = bytes(caps_f32.len());
        let adaptation_bytes = bytes(adaptation_f32.len());
        let opponent_bytes = bytes(opponent_f32.len());
        let rout_bytes = bytes(n_out * n_pixels);

        let mut guard = self.receptor_pool.lock().unwrap();
        if guard.is_none() {
            *guard = Some(ReceptorPool::new(
                self,
                cube_bytes,
                response_bytes,
                gains_bytes,
                caps_bytes,
                adaptation_bytes.max(4),
                opponent_bytes.max(4),
                rout_bytes,
            ));
        }
        let pool = guard.as_mut().unwrap();

        // Grow whichever buffers this call's shape needs, and rebuild the
        // bind group whenever any handle it captures is (re)allocated.
        let mut rebuild = false;
        if pool.cube_cap < cube_bytes {
            pool.cube_cap = pooled_capacity(cube_bytes);
            pool.cube = alloc_buffer(
                &self.device,
                "receptor_cube",
                pool.cube_cap,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            rebuild = true;
        }
        if pool.response_cap < response_bytes {
            pool.response_cap = pooled_capacity(response_bytes);
            pool.response = alloc_buffer(
                &self.device,
                "receptor_response",
                pool.response_cap,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            rebuild = true;
        }
        if pool.gains_cap < gains_bytes {
            pool.gains_cap = pooled_capacity(gains_bytes);
            pool.gains = alloc_buffer(
                &self.device,
                "receptor_gains",
                pool.gains_cap,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            rebuild = true;
        }
        if pool.caps_cap < caps_bytes {
            pool.caps_cap = pooled_capacity(caps_bytes);
            pool.caps = alloc_buffer(
                &self.device,
                "receptor_caps",
                pool.caps_cap,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            rebuild = true;
        }
        if pool.adaptation_cap < adaptation_bytes.max(4) {
            pool.adaptation_cap = pooled_capacity(adaptation_bytes.max(4));
            pool.adaptation = alloc_buffer(
                &self.device,
                "receptor_adaptation",
                pool.adaptation_cap,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            rebuild = true;
        }
        if pool.opponent_cap < opponent_bytes.max(4) {
            pool.opponent_cap = pooled_capacity(opponent_bytes.max(4));
            pool.opponent = alloc_buffer(
                &self.device,
                "receptor_opponent",
                pool.opponent_cap,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            rebuild = true;
        }
        if pool.rout_cap < rout_bytes {
            pool.rout_cap = pooled_capacity(rout_bytes);
            pool.rout = alloc_buffer(
                &self.device,
                "receptor_rout",
                pool.rout_cap,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            );
            rebuild = true;
        }
        if pool.readback_cap < rout_bytes {
            pool.readback_cap = pooled_capacity(rout_bytes);
            pool.readback = alloc_buffer(
                &self.device,
                "receptor_readback",
                pool.readback_cap,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            );
            // readback is in no bind group, so it never forces a rebuild.
        }
        if rebuild {
            pool.bg = make_receptor_bind_group(
                self,
                &pool.params,
                &pool.cube,
                &pool.response,
                &pool.gains,
                &pool.caps,
                &pool.adaptation,
                &pool.opponent,
                &pool.rout,
            );
        }

        // Only the per-call input contents are (re)uploaded.
        self.queue
            .write_buffer(&pool.params, 0, bytemuck::bytes_of(&params));
        self.queue
            .write_buffer(&pool.cube, 0, bytemuck::cast_slice(cube_region));
        self.queue
            .write_buffer(&pool.response, 0, bytemuck::cast_slice(&response_f32));
        self.queue
            .write_buffer(&pool.gains, 0, bytemuck::cast_slice(&gains_f32));
        self.queue
            .write_buffer(&pool.caps, 0, bytemuck::cast_slice(&caps_f32));
        if !adaptation_f32.is_empty() {
            self.queue
                .write_buffer(&pool.adaptation, 0, bytemuck::cast_slice(&adaptation_f32));
        }
        if !opponent_f32.is_empty() {
            self.queue
                .write_buffer(&pool.opponent, 0, bytemuck::cast_slice(&opponent_f32));
        }

        // 2-D dispatch: x is capped at 65535 workgroups (wgpu's per-dimension
        // limit), so each y slice covers at most 65535*64 = 4,194,240
        // pixels - exactly what the shader's `gid.y * PIXELS_PER_SLICE`
        // assumes - and y covers however many such slices are needed.
        let total_wgs = (n_pixels as u64).div_ceil(64);
        let x = total_wgs.min(65535) as u32;
        let y = total_wgs.div_ceil(x as u64) as u32;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&self.receptor_pipeline);
            pass.set_bind_group(0, &pool.bg, &[]);
            pass.dispatch_workgroups(x, y, 1);
        }
        encoder.copy_buffer_to_buffer(&pool.rout, 0, &pool.readback, 0, rout_bytes);
        self.queue.submit(Some(encoder.finish()));

        let slice = pool.readback.slice(..rout_bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        if rx.recv().ok()?.is_err() {
            return None;
        }
        let out = match slice.get_mapped_range() {
            Ok(mapped) => {
                let floats: &[f32] = bytemuck::cast_slice(&mapped);
                floats.to_vec()
            }
            Err(_) => return None,
        };
        // Unmap so the pooled readback buffer is reusable next call.
        pool.readback.unmap();
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_sum_batch_matches_hand_calculation() {
        let Some(ctx) = try_init() else {
            eprintln!("no GPU adapter available in this environment - skipping");
            return;
        };
        // Same 3-curve, 2-batch case verified by hand before this was
        // ported from the throwaway probe: curves [1,2,3,4]/[4,3,2,1]/
        // [1,1,1,1], batch 0 = identity (offdiag all 0), batch 1 =
        // offdiag all 0.5. Hand-calculated overlap_sum (sum of squared
        // pairwise overlaps):
        // batch 0: 20^2 + 10^2 + 10^2 = 400+100+100 = 600
        // batch 1: 71^2 + 59.5^2 + 59.5^2 = 5041+3540.25+3540.25=12121.5
        let sampled = vec![
            vec![1.0, 2.0, 3.0, 4.0],
            vec![4.0, 3.0, 2.0, 1.0],
            vec![1.0, 1.0, 1.0, 1.0],
        ];
        let offdiag_batch = vec![vec![0.0; 6], vec![0.5; 6]];
        let result = ctx
            .overlap_sum_batch(&sampled, 3, 1.0, &offdiag_batch)
            .expect("GPU dispatch should succeed in this test");
        assert_eq!(result.len(), 2);
        assert!((result[0] - 600.0).abs() < 0.1, "batch0={}", result[0]);
        assert!((result[1] - 12121.5).abs() < 1.0, "batch1={}", result[1]);
    }

    /// CPU kernel mirroring `transform_batch` + `overlap_batch` exactly:
    /// transform the sampled curves by the diagonal-1 matrix built from
    /// `offdiag`, then sum squared, step-scaled, pairwise overlaps. This is
    /// the per-candidate work the shader batches over.
    fn cpu_overlap_sum(sampled: &[Vec<f64>], n: usize, step_nm: f64, offdiag: &[f64]) -> f64 {
        let grid_len = sampled[0].len();
        let mut m = vec![0.0f64; n * n];
        let mut k = 0;
        for i in 0..n {
            for j in 0..n {
                m[i * n + j] = if i == j {
                    1.0
                } else {
                    let v = offdiag[k];
                    k += 1;
                    v
                };
            }
        }
        let mut transformed = vec![vec![0.0f64; grid_len]; n];
        for i in 0..n {
            for kk in 0..n {
                let w = m[i * n + kk];
                if w != 0.0 {
                    let src = &sampled[kk];
                    let dst = &mut transformed[i];
                    for g in 0..grid_len {
                        dst[g] += w * src[g];
                    }
                }
            }
        }
        let mut sum = 0.0;
        for i in 0..n {
            for j in (i + 1)..n {
                let mut d = 0.0;
                for g in 0..grid_len {
                    d += transformed[i][g] * transformed[j][g];
                }
                let ov = d * step_nm;
                sum += ov * ov;
            }
        }
        sum
    }

    /// The method that actually replaced the GPU path: build the Gram
    /// matrix once (`O(n^2 * grid)`) then take one analytic objective +
    /// gradient pass (`O(n^3)`) - instead of the `1 + 2*(n^2-n)` separate
    /// objective evaluations one finite-difference gradient step needs.
    fn cpu_exact_gradient_step(sampled: &[Vec<f64>], n: usize, step_nm: f64, reg: f64) -> f64 {
        let mut g = vec![0.0f64; n * n];
        for a in 0..n {
            for b in a..n {
                let d: f64 = sampled[a]
                    .iter()
                    .zip(&sampled[b])
                    .map(|(x, y)| x * y)
                    .sum();
                let v = d * step_nm;
                g[a * n + b] = v;
                g[b * n + a] = v;
            }
        }
        let offdiag: Vec<f64> = (0..n * n - n)
            .map(|idx| ((idx as f64) * 0.37).sin() * 0.3)
            .collect();
        let mut m = vec![0.0f64; n * n];
        let mut k = 0;
        for i in 0..n {
            for j in 0..n {
                m[i * n + j] = if i == j {
                    1.0
                } else {
                    let v = offdiag[k];
                    k += 1;
                    v
                };
            }
        }
        let mg = mat_mul(&m, &g, n);
        let mut mt = vec![0.0f64; n * n];
        for i in 0..n {
            for j in 0..n {
                mt[i * n + j] = m[j * n + i];
            }
        }
        let o = mat_mul(&mg, &mt, n);
        let mut f = 0.0;
        for i in 0..n {
            for j in (i + 1)..n {
                f += o[i * n + j] * o[i * n + j];
            }
        }
        f += offdiag.iter().map(|p| p * p).sum::<f64>() * reg;
        // Analytic gradient terms (the `O(n^3)` part of a step).
        let mut grad_sum = 0.0;
        for a in 0..n {
            for b in 0..n {
                if a == b {
                    continue;
                }
                let mut s = 0.0;
                for j in 0..n {
                    if j != a {
                        s += o[a * n + j] * mg[j * n + b];
                    }
                }
                grad_sum += 2.0 * s + 2.0 * reg * m[a * n + b];
            }
        }
        f + grad_sum.abs() * 0.0
    }

    fn mat_mul(a: &[f64], b: &[f64], n: usize) -> Vec<f64> {
        let mut out = vec![0.0f64; n * n];
        for i in 0..n {
            for j in 0..n {
                let mut s = 0.0;
                for k in 0..n {
                    s += a[i * n + k] * b[k * n + j];
                }
                out[i * n + j] = s;
            }
        }
        out
    }

    /// Evenly spaced Gaussian stand-ins for a species' receptors across a
    /// common grid, as raw sampled vectors (no `SpectralCurve` needed).
    fn gaussian_curves(n: usize, grid_len: usize) -> Vec<Vec<f64>> {
        let sigma = 45.0;
        (0..n)
            .map(|i| {
                let mu = if n > 1 {
                    let frac = i as f64 / (n - 1) as f64;
                    (50.0 + frac * (grid_len as f64 - 100.0)).round()
                } else {
                    (grid_len / 2) as f64
                };
                (0..grid_len)
                    .map(|g| {
                        let d = (g as f64 - mu) / sigma;
                        (-0.5 * d * d).exp()
                    })
                    .collect()
            })
            .collect()
    }

    /// Not run by default (`cargo test -p xenovision-core --release -- --ignored --nocapture`
    /// to reproduce). Times the workload the GPU path existed for - one
    /// finite-difference gradient step, i.e. `1 + 2*(N^2 - N)` objective
    /// evaluations of a single species' receptor curves - on the CPU (all
    /// candidates sequentially) versus one batched GPU dispatch pair, and
    /// compares that to the analytic-gradient CPU step that replaced it.
    /// Receptors are Gaussian stand-ins evenly spaced over a 451-sample
    /// (300-750nm @ 1nm) grid.
    #[test]
    #[ignore]
    fn bench_cpu_vs_gpu_overlap_sum() {
        let Some(ctx) = try_init() else {
            eprintln!("no GPU adapter available in this environment - skipping");
            return;
        };
        let grid_len = 451usize;
        let step_nm = 1.0;
        let reps = 20;

        println!("\n=== CPU vs GPU: one finite-difference gradient step ===");
        println!("batch = 1 + 2*(N^2 - N) candidate matrices; grid = {grid_len} samples");
        println!(
            "{:<3} {:>7} {:>11} {:>11} {:>9} {:>15}",
            "N", "batch", "CPU [ms]", "GPU [ms]", "CPU/GPU", "exact-grad [ms]"
        );

        for &n in &[3usize, 5, 8, 12, 16] {
            let sampled = gaussian_curves(n, grid_len);
            let batch = 1 + 2 * (n * n - n);
            let candidates: Vec<Vec<f64>> = (0..batch)
                .map(|b| {
                    (0..n * n - n)
                        .map(|kk| (((b * 131 + kk) as f64) * 0.017).sin() * 0.3)
                        .collect()
                })
                .collect();

            let mut cpu_best = f64::MAX;
            for _ in 0..reps {
                let t = std::time::Instant::now();
                let mut acc = 0.0;
                for c in &candidates {
                    acc += cpu_overlap_sum(&sampled, n, step_nm, c);
                }
                std::hint::black_box(acc);
                cpu_best = cpu_best.min(t.elapsed().as_secs_f64());
            }

            // Warm up: the first call allocates the pooled buffers (and
            // grows them whenever N changes), so time only the steady
            // state where every buffer is already available and reused.
            let _ = std::hint::black_box(ctx.overlap_sum_batch(&sampled, n, step_nm, &candidates));

            let mut gpu_best = f64::MAX;
            for _ in 0..reps {
                let t = std::time::Instant::now();
                let r = ctx.overlap_sum_batch(&sampled, n, step_nm, &candidates);
                std::hint::black_box(r);
                gpu_best = gpu_best.min(t.elapsed().as_secs_f64());
            }

            let mut eg_best = f64::MAX;
            for _ in 0..reps {
                let t = std::time::Instant::now();
                let v = cpu_exact_gradient_step(&sampled, n, step_nm, 0.0);
                std::hint::black_box(v);
                eg_best = eg_best.min(t.elapsed().as_secs_f64());
            }

            println!(
                "{:<3} {:>7} {:>11.3} {:>11.3} {:>9.2} {:>15.4}",
                n,
                batch,
                cpu_best * 1e3,
                gpu_best * 1e3,
                cpu_best / gpu_best,
                eg_best * 1e3
            );
        }
    }
}
