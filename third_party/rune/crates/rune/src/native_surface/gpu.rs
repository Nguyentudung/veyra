//! GPU-native frames for [`super::NativeSurface`].
//!
//! A producer renders into a texture on Rune's `wgpu::Device`, submits to the
//! same queue, then publishes a `GpuFrame`. The surface renderer samples that
//! view directly. The existing completion callback retains `Imported` until
//! the sampling submission completes, so a producer-owned lease is released
//! only after the GPU is done with the slot.

use super::{ColorInfo, Format};
use std::{
    any::Any,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock, PoisonError, RwLock,
    },
    time::{Duration, Instant},
};

const DIAGNOSTIC_INTERVAL: Duration = Duration::from_secs(1);

/// CPU-side compositor counters shared by Rune and a GPU-native video producer.
///
/// All timings measure host work only. They deliberately say nothing about GPU
/// execution time or completion. Counters are sampled with atomic swaps, so a
/// concurrent upload can fall into the adjacent reporting interval.
#[derive(Clone)]
pub struct CompositorDiagnostics(Arc<CompositorDiagnosticsInner>);

struct CompositorDiagnosticsInner {
    started: Instant,
    last_report_ns: AtomicU64,
    renderer: OnceLock<&'static str>,
    frames_drawn: AtomicU64,
    encoders_created: AtomicU64,
    submissions: AtomicU64,
    main_render_passes: AtomicU64,
    write_texture_calls: AtomicU64,
    prepare_ns: AtomicU64,
    draw_ns: AtomicU64,
    submit_ns: AtomicU64,
}

/// One approximately one-second interval of compositor CPU activity.
pub struct CompositorInterval {
    pub renderer: &'static str,
    pub elapsed: Duration,
    pub frames_drawn: u64,
    pub encoders_created: u64,
    pub submissions: u64,
    pub main_render_passes: u64,
    pub write_texture_calls: u64,
    pub prepare: Duration,
    pub draw: Duration,
    pub submit: Duration,
}

impl CompositorDiagnostics {
    pub(crate) fn new() -> Self {
        Self(Arc::new(CompositorDiagnosticsInner {
            started: Instant::now(),
            last_report_ns: AtomicU64::new(0),
            renderer: OnceLock::new(),
            frames_drawn: AtomicU64::new(0),
            encoders_created: AtomicU64::new(0),
            submissions: AtomicU64::new(0),
            main_render_passes: AtomicU64::new(0),
            write_texture_calls: AtomicU64::new(0),
            prepare_ns: AtomicU64::new(0),
            draw_ns: AtomicU64::new(0),
            submit_ns: AtomicU64::new(0),
        }))
    }

    /// Labels diagnostics with the active producer mode. The first label wins
    /// because one compositor/device has one active video producer at a time.
    pub fn set_renderer(&self, renderer: &'static str) {
        let _ = self.0.renderer.set(renderer);
    }

    pub fn record_encoder_created(&self) {
        self.0.encoders_created.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_frame_drawn(&self) {
        self.0.frames_drawn.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_main_render_pass(&self) {
        self.0.main_render_passes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_write_texture(&self) {
        self.0.write_texture_calls.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_prepare(&self, elapsed: Duration) {
        self.add_duration(&self.0.prepare_ns, elapsed);
    }

    pub fn record_draw(&self, elapsed: Duration) {
        self.add_duration(&self.0.draw_ns, elapsed);
    }

    pub fn record_submit(&self, elapsed: Duration) {
        self.0.submissions.fetch_add(1, Ordering::Relaxed);
        self.add_duration(&self.0.submit_ns, elapsed);
    }

    fn add_duration(&self, target: &AtomicU64, elapsed: Duration) {
        target.fetch_add(elapsed.as_nanos().min(u64::MAX as u128) as u64, Ordering::Relaxed);
    }

    /// Returns a counter snapshot at most once per approximate second.
    pub fn take_interval(&self) -> Option<CompositorInterval> {
        let now_ns = self
            .0
            .started
            .elapsed()
            .as_nanos()
            .min(u64::MAX as u128) as u64;
        let previous = self.0.last_report_ns.load(Ordering::Relaxed);
        if now_ns.saturating_sub(previous) < DIAGNOSTIC_INTERVAL.as_nanos() as u64
            || self
                .0
                .last_report_ns
                .compare_exchange(previous, now_ns, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            return None;
        }
        Some(CompositorInterval {
            renderer: self.0.renderer.get().copied().unwrap_or("native-surface"),
            elapsed: Duration::from_nanos(now_ns.saturating_sub(previous)),
            frames_drawn: self.0.frames_drawn.swap(0, Ordering::Relaxed),
            encoders_created: self.0.encoders_created.swap(0, Ordering::Relaxed),
            submissions: self.0.submissions.swap(0, Ordering::Relaxed),
            main_render_passes: self.0.main_render_passes.swap(0, Ordering::Relaxed),
            write_texture_calls: self.0.write_texture_calls.swap(0, Ordering::Relaxed),
            prepare: Duration::from_nanos(self.0.prepare_ns.swap(0, Ordering::Relaxed)),
            draw: Duration::from_nanos(self.0.draw_ns.swap(0, Ordering::Relaxed)),
            submit: Duration::from_nanos(self.0.submit_ns.swap(0, Ordering::Relaxed)),
        })
    }
}

#[derive(Clone)]
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub generation: u64,
    pub compositor_diagnostics: CompositorDiagnostics,
}

impl GpuContext {
    #[cfg(test)]
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue, generation: u64) -> Self {
        Self::with_diagnostics(device, queue, generation, CompositorDiagnostics::new())
    }

    pub(crate) fn with_diagnostics(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        generation: u64,
        compositor_diagnostics: CompositorDiagnostics,
    ) -> Self {
        Self {
            device: device.clone(),
            queue: queue.clone(),
            generation,
            compositor_diagnostics,
        }
    }
}

/// A BGRA texture produced on Rune's device and queue.
pub struct GpuFrame {
    pub view: wgpu::TextureView,
    pub size_px: [u32; 2],
    pub generation: u64,
    /// Identifies an output-texture pool generation and its slot for Rune's
    /// bind-group cache. A resize must use a new pool id.
    pub pool_id: u64,
    pub slot: u32,
    /// Producer-defined RAII lease. Its drop must not call wgpu: it can run on
    /// wgpu's completion callback thread.
    pub lease: Arc<dyn Any + Send + Sync>,
}

/// A two-plane NV12 picture produced on Rune's device and queue.
///
/// Plane 0 is `R8Unorm` luma at `size_px`; plane 1 is `Rg8Unorm` interleaved
/// Cb/Cr at half resolution. Rune samples these views in its existing video
/// compositor, avoiding an intermediate producer-side RGB render pass.
pub struct GpuVideoFrame {
    pub planes: [wgpu::TextureView; 2],
    pub size_px: [u32; 2],
    pub format: Format,
    pub color: ColorInfo,
    pub generation: u64,
    /// Identifies a producer pool generation and slot for Rune's bind cache.
    pub pool_id: u64,
    pub slot: u32,
    /// Producer-defined RAII lease. As for [`GpuFrame`], its drop must not
    /// make wgpu calls because it can run in a completion callback.
    pub lease: Arc<dyn Any + Send + Sync>,
}

#[derive(Default)]
pub(crate) struct GpuShared {
    context: RwLock<Option<GpuContext>>,
}

impl GpuShared {
    pub(crate) fn publish(&self, context: &GpuContext) {
        let stale = self
            .context
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_none_or(|old| old.generation != context.generation);
        if stale {
            *self.context.write().unwrap_or_else(PoisonError::into_inner) = Some(context.clone());
        }
    }
    pub(crate) fn context(&self) -> Option<GpuContext> {
        self.context
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn generation(&self) -> Option<u64> {
        self.context
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|context| context.generation)
    }
}
