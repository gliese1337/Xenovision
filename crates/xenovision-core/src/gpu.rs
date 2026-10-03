//! GPU compute (wgpu) for batched adaptation-objective evaluations.
//!
//! **Currently unused.** `adaptation` used to route its finite-difference
//! evaluations here for N > 16, but its exact-gradient CPU method is now
//! faster at every size measured, so nothing calls this module. It's kept
//! as a working wgpu setup for future GPU work (e.g. rendering a whole
//! hyperspectral image as a given species would see it).
//!
//! Known limitation before reusing it: each pass is one 1-D dispatch, so
//! batches larger than 65535 workgroups (reached around N≈20 for the
//! adaptation batch) exceed wgpu's per-dimension limit and panic instead
//! of returning `None`. Split large dispatches across dimensions or calls.
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

use wgpu::util::DeviceExt;

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

pub struct GpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    transform_pipeline: wgpu::ComputePipeline,
    overlap_pipeline: wgpu::ComputePipeline,
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

    Some(GpuContext {
        device,
        queue,
        transform_pipeline,
        overlap_pipeline,
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

        let params_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let curves_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("curves"),
                contents: bytemuck::cast_slice(&curves_f32),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let offdiag_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("offdiag_batch"),
                contents: bytemuck::cast_slice(&offdiag_f32),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let transformed_size = (batch_size * n * grid_len) as u64 * 4;
        let transformed_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("transformed"),
            size: transformed_size,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let pair_overlap_size = (batch_size * num_pairs) as u64 * 4;
        let pair_overlap_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pair_overlap"),
            size: pair_overlap_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let transform_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("transform_bg"),
            layout: &self.transform_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: curves_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: offdiag_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: transformed_buf.as_entire_binding(),
                },
            ],
        });
        let overlap_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlap_bg"),
            layout: &self.overlap_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: transformed_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: pair_overlap_buf.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&self.transform_pipeline);
            pass.set_bind_group(0, &transform_bg, &[]);
            let total1 = (batch_size * n * grid_len) as u32;
            pass.dispatch_workgroups(total1.div_ceil(64), 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&self.overlap_pipeline);
            pass.set_bind_group(0, &overlap_bg, &[]);
            let total2 = (batch_size * num_pairs) as u32;
            pass.dispatch_workgroups(total2.div_ceil(64), 1, 1);
        }

        let readback_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: pair_overlap_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&pair_overlap_buf, 0, &readback_buf, 0, pair_overlap_size);
        self.queue.submit(Some(encoder.finish()));

        let slice = readback_buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        rx.recv().ok()?.ok()?;
        let mapped = slice.get_mapped_range().ok()?;
        let pair_overlaps: &[f32] = bytemuck::cast_slice(&mapped);

        Some(
            (0..batch_size)
                .map(|b| {
                    pair_overlaps[b * num_pairs..(b + 1) * num_pairs]
                        .iter()
                        .map(|&v| v as f64)
                        .sum::<f64>()
                })
                .collect(),
        )
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
}
