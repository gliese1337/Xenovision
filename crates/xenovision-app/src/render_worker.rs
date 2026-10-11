//! Background renderer for the Image Display window.
//!
//! Rendering a cube through a visual system's receptor channels is the one
//! genuinely heavy operation the GUI performs, and it scales with the
//! image size (one receptor×band matrix multiply per pixel on the CPU; an
//! upload/dispatch/readback round-trip on the GPU). Doing it on the UI
//! thread stalls the whole window for the duration, so it runs on a single
//! long-lived worker thread instead:
//!
//! - the UI thread builds a [`RenderJob`] (cheap - the precomputed
//!   [`ReceptorKernel`] plus an `Arc` handle to the cube, no pixel copy)
//!   and submits it,
//! - the worker renders (GPU when a context is supplied, else CPU) and
//!   sends back a [`RenderOutcome`],
//! - the UI thread drains finished outcomes each frame and installs the
//!   ones that still match the view's current inputs, dropping stale ones.
//!
//! One worker, not a pool: a single Image Display renders a handful of
//! views that all share the one GPU context, whose receptor pool
//! serializes GPU work anyway, and [`ReceptorKernel::render`] already
//! parallelizes across pixel rows internally.
//!
//! Jobs carry an `Arc<GpuContext>` when the GPU is available, keeping the
//! context alive for the worker even as the app tears down.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use xenovision_core::gpu::GpuContext;
use xenovision_core::hyperspectral::{HyperspectralCube, Region};
use xenovision_core::{ReceptorImage, ReceptorKernel};

/// One unit of work: everything needed to render one view off-thread.
pub struct RenderJob {
    /// The `ImageView::id` this render belongs to (matched back on the UI
    /// thread; a view that has since been removed is simply dropped).
    pub view_id: u64,
    /// The view's cache key at submission. Only an outcome whose key still
    /// matches the view's pending key is installed, so a result that
    /// arrived after the inputs changed again is discarded.
    pub key: String,
    /// The source cube, shared with the library (no pixel copy).
    pub cube: Arc<HyperspectralCube>,
    /// The precomputed render kernel (built on the UI thread, where the
    /// app's curve-set types live).
    pub kernel: ReceptorKernel,
    /// Region to render, or `None` for the whole image.
    pub region: Option<Region>,
    /// GPU context when available; `None` renders on the CPU.
    pub gpu: Option<Arc<GpuContext>>,
}

/// A finished render, tagged so the UI thread can match it to its view.
pub struct RenderOutcome {
    pub view_id: u64,
    pub key: String,
    pub result: Result<RenderProduct, String>,
}

/// The rendered channels plus whether the GPU path was used (for the
/// provenance bar).
pub struct RenderProduct {
    pub image: ReceptorImage,
    pub used_gpu: bool,
}

/// Handle to the render worker. Dropping it closes the job channel, drains
/// the job in progress, then joins the worker, so no render (nor the
/// `Arc<GpuContext>`/cube it borrows) outlives the state that owns it.
pub struct RenderEngine {
    jobs: Option<Sender<RenderJob>>,
    outcomes: Receiver<RenderOutcome>,
    worker: Option<thread::JoinHandle<()>>,
}

impl RenderEngine {
    pub fn new() -> Self {
        let (job_tx, job_rx) = mpsc::channel::<RenderJob>();
        let (out_tx, out_rx) = mpsc::channel::<RenderOutcome>();
        let worker = thread::Builder::new()
            .name("xenovision-render".to_string())
            .spawn(move || worker_loop(job_rx, out_tx))
            .ok();
        RenderEngine {
            jobs: Some(job_tx),
            outcomes: out_rx,
            worker,
        }
    }

    /// Queues a job. Never blocks (the channel is unbounded); a send error
    /// means the worker died, which the caller treats as a dropped render.
    pub fn submit(&self, job: RenderJob) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
    }

    /// Non-blocking: the next finished render, if any.
    pub fn try_recv(&self) -> Option<RenderOutcome> {
        self.outcomes.try_recv().ok()
    }
}

impl Default for RenderEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RenderEngine {
    fn drop(&mut self) {
        // Closing the job channel makes the worker's `recv` fail once it
        // finishes the job it is on; joining then guarantees the thread
        // (and anything it holds) is gone before the owning state drops.
        self.jobs = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn worker_loop(jobs: Receiver<RenderJob>, outcomes: Sender<RenderOutcome>) {
    // Jobs that are queued but not yet started, at most one per view: a
    // newer job for a view replaces the older one, so a render superseded
    // before the worker reaches it is dropped instead of computed. (A
    // render already in progress runs to completion - core rendering has no
    // mid-pass cancellation - but its result is discarded as stale.)
    let mut queued: Vec<RenderJob> = Vec::new();
    loop {
        if queued.is_empty() {
            // Nothing buffered: block until the next job (or shutdown).
            match jobs.recv() {
                Ok(job) => queued.push(job),
                Err(_) => break, // UI dropped its sender
            }
        }
        // Absorb everything currently ready, coalescing per view. Re-run on
        // every iteration so a job that becomes superseded while the
        // previous one renders never starts.
        while let Ok(job) = jobs.try_recv() {
            supersede(&mut queued, job);
        }

        let job = queued.remove(0);
        let result = render(&job);
        let outcome = RenderOutcome {
            view_id: job.view_id,
            key: job.key,
            result,
        };
        if outcomes.send(outcome).is_err() {
            // The UI thread dropped its receiver; nothing left to do.
            break;
        }
    }
}

/// Inserts `job` into the pending `queued` list, replacing any earlier job
/// for the same view (so the older, superseded one is cancelled). A new
/// view is appended, preserving first-seen order.
fn supersede(queued: &mut Vec<RenderJob>, job: RenderJob) {
    if let Some(slot) = queued.iter_mut().find(|j| j.view_id == job.view_id) {
        *slot = job;
    } else {
        queued.push(job);
    }
}

fn render(job: &RenderJob) -> Result<RenderProduct, String> {
    let cube = job.cube.as_ref();
    let region = job.region.as_ref();
    let cpu = || {
        job.kernel
            .render(cube, region)
            .map(|image| RenderProduct {
                image,
                used_gpu: false,
            })
            .map_err(|e| e.to_string())
    };
    match &job.gpu {
        Some(gpu) => match job.kernel.render_gpu(gpu, cube, region) {
            Some(image) => Ok(RenderProduct {
                image,
                used_gpu: true,
            }),
            None => cpu(),
        },
        None => cpu(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use xenovision_core::InputKind;

    fn sample_cube() -> HyperspectralCube {
        let (samples, lines, bands) = (3usize, 2usize, 3usize);
        HyperspectralCube::from_band_major(
            samples,
            lines,
            bands,
            Some(vec![450.0, 550.0, 650.0]),
            vec![0.5f32; samples * lines * bands],
            "synthetic",
        )
    }

    /// A pool kernel over a real fixture set, radiance-in (so no luminant
    /// is required) - enough to exercise the worker end to end.
    fn sample_kernel() -> ReceptorKernel {
        let app = crate::state::AppState::new();
        let set_id = app.workspace.open_tabs[0];
        let core = app.curve_sets[&set_id].to_core();
        let wavelengths = vec![450.0, 550.0, 650.0];
        ReceptorKernel::for_pool(&core, &wavelengths, None, InputKind::Radiance, 1.0)
            .expect("kernel builds")
    }

    fn wait_for(engine: &RenderEngine) -> RenderOutcome {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(outcome) = engine.try_recv() {
                return outcome;
            }
            assert!(Instant::now() < deadline, "render did not finish in time");
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// A minimal real job, for exercising queue bookkeeping without caring
    /// about what it returns.
    fn test_job(view_id: u64, key: &str) -> RenderJob {
        RenderJob {
            view_id,
            key: key.to_string(),
            cube: Arc::new(sample_cube()),
            kernel: sample_kernel(),
            region: None,
            gpu: None,
        }
    }

    /// A superseded job (same view, newer key) replaces the queued one so it
    /// is never rendered; distinct views each keep their newest job, in
    /// first-seen order.
    #[test]
    fn supersede_keeps_only_the_newest_job_per_view() {
        let mut queued = Vec::new();
        supersede(&mut queued, test_job(1, "a"));
        supersede(&mut queued, test_job(2, "b"));
        supersede(&mut queued, test_job(1, "c")); // cancels (1, "a")
        supersede(&mut queued, test_job(2, "d")); // cancels (2, "b")
        supersede(&mut queued, test_job(1, "e")); // cancels (1, "c")

        let seen: Vec<(u64, String)> = queued.iter().map(|j| (j.view_id, j.key.clone())).collect();
        assert_eq!(
            seen,
            vec![(1, "e".to_string()), (2, "d".to_string())],
            "first-seen view order preserved, newest job kept per view"
        );
    }

    #[test]
    fn renders_a_job_on_the_worker_and_reports_cpu() {
        let engine = RenderEngine::new();
        engine.submit(RenderJob {
            view_id: 7,
            key: "k".to_string(),
            cube: Arc::new(sample_cube()),
            kernel: sample_kernel(),
            region: None,
            gpu: None,
        });

        let outcome = wait_for(&engine);
        assert_eq!(outcome.view_id, 7);
        assert_eq!(outcome.key, "k");
        let product = outcome.result.expect("job renders");
        assert!(!product.used_gpu, "no GPU context was supplied");
        assert_eq!(product.image.width, 3);
        assert_eq!(product.image.height, 2);
    }

    #[test]
    fn outcomes_are_tagged_with_their_job() {
        let cube = Arc::new(sample_cube());
        let kernel = sample_kernel();
        let engine = RenderEngine::new();
        for (id, key) in [(1u64, "a"), (2u64, "b")] {
            engine.submit(RenderJob {
                view_id: id,
                key: key.to_string(),
                cube: cube.clone(),
                kernel: kernel.clone(),
                region: None,
                gpu: None,
            });
        }

        let mut seen: Vec<(u64, String)> = (0..2)
            .map(|_| {
                let outcome = wait_for(&engine);
                (outcome.view_id, outcome.key)
            })
            .collect();
        seen.sort();
        assert_eq!(seen, vec![(1, "a".to_string()), (2, "b".to_string())]);
    }

    #[test]
    fn dropping_the_engine_joins_the_worker() {
        // A submitted-then-dropped engine must not hang or leak: the drop
        // drains the in-flight job and joins.
        let engine = RenderEngine::new();
        engine.submit(RenderJob {
            view_id: 1,
            key: "k".to_string(),
            cube: Arc::new(sample_cube()),
            kernel: sample_kernel(),
            region: None,
            gpu: None,
        });
        drop(engine);
    }
}
