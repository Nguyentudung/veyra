use super::super::ffi;
use super::{frame::VideoFrame, renderer::VideoRenderer};
use crate::FrameMailbox;
use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

pub(crate) struct PresentationClock {
    started_at: Option<Instant>,
    first_pts: Option<f64>,
    diagnostics: DiagnosticMode,
    diagnostics_frames: u64,
    diagnostics_started: Instant,
}

/// Opt-in probes for locating failures at the software presentation boundary.
/// They deliberately preserve demuxing, decoding and PTS pacing. None of these
/// modes is used during ordinary playback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiagnosticMode {
    Disabled,
    DecodeOnly,
    AllocOnly,
    ScaleOnly,
    ScaleGuard,
    RenderOnly,
}

impl DiagnosticMode {
    fn from_environment() -> Self {
        match std::env::var("VEYRA_VIDEO_DIAGNOSTICS").as_deref() {
            Ok("decode-only") => Self::DecodeOnly,
            Ok("alloc-only") => Self::AllocOnly,
            Ok("scale-only") => Self::ScaleOnly,
            Ok("scale-guard") => Self::ScaleGuard,
            // Explicitly exercises the same conversion and presentation path
            // as normal playback, while labelling it in the diagnostic output.
            Ok("render-only") => Self::RenderOnly,
            _ => Self::Disabled,
        }
    }

    fn label(self) -> Option<&'static str> {
        match self {
            Self::Disabled => None,
            Self::DecodeOnly => Some("decode-only"),
            Self::AllocOnly => Some("alloc-only"),
            Self::ScaleOnly => Some("scale-only"),
            Self::ScaleGuard => Some("scale-guard"),
            Self::RenderOnly => Some("render-only"),
        }
    }
}

impl PresentationClock {
    pub(crate) fn new() -> Self {
        let diagnostics = DiagnosticMode::from_environment();
        if let Some(mode) = diagnostics.label() {
            eprintln!("[video] diagnostics={mode}");
        }
        Self {
            started_at: None,
            first_pts: None,
            diagnostics,
            diagnostics_frames: 0,
            diagnostics_started: Instant::now(),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.started_at = None;
        self.first_pts = None;
    }

    fn wait_until(&mut self, pts: f64) {
        let now = Instant::now();
        let first_pts = *self.first_pts.get_or_insert(pts);
        let started_at = *self.started_at.get_or_insert(now);
        let target = started_at + Duration::from_secs_f64((pts - first_pts).max(0.0));
        if let Some(remaining) = target.checked_duration_since(now) {
            thread::sleep(remaining);
        }
    }

    fn diagnostics(&self) -> DiagnosticMode {
        self.diagnostics
    }

    /// Reports diagnostic frames at a low rate without adding work per frame.
    fn observe_diagnostic_frame(&mut self, frame: *const ffi::AVFrame) {
        self.diagnostics_frames = self.diagnostics_frames.saturating_add(1);
        let elapsed = self.diagnostics_started.elapsed();
        if elapsed < Duration::from_secs(1) {
            return;
        }
        unsafe {
            eprintln!(
                "[video][{}] interval_ms={:.0} decoded_frames={} size={}x{} format={} strides=[{}, {}, {}]",
                self.diagnostics.label().unwrap_or("disabled"),
                elapsed.as_secs_f64() * 1000.0,
                self.diagnostics_frames,
                (*frame).width,
                (*frame).height,
                (*frame).format,
                (*frame).linesize[0],
                (*frame).linesize[1],
                (*frame).linesize[2],
            );
        }
        self.diagnostics_frames = 0;
        self.diagnostics_started = Instant::now();
    }
}

pub(crate) unsafe fn decode_video(
    context: *mut ffi::AVCodecContext,
    packet: *mut ffi::AVPacket,
    frame: *mut ffi::AVFrame,
    renderer: &mut VideoRenderer,
    output: &Arc<FrameMailbox>,
    snapshot: &Arc<Mutex<super::super::state::Snapshot>>,
    time_base: ffi::AVRational,
    clock: &mut PresentationClock,
) -> Result<(), String> {
    check(
        ffi::avcodec_send_packet(context, packet),
        "send video packet",
    )?;
    while ffi::avcodec_receive_frame(context, frame) >= 0 {
        let timestamp = timestamp_seconds(frame, time_base);
        // Pace decoding by the source PTS, independently of when an async GPU
        // readback completes. Otherwise all slots could fill before the clock
        // has a chance to slow the decoder down.
        clock.wait_until(timestamp);
        let processing_started = Instant::now();
        let rendered = match clock.diagnostics() {
            // This probe does not touch the renderer at all.
            DiagnosticMode::DecodeOnly => {
                clock.observe_diagnostic_frame(frame);
                None
            }
            // Match the software renderer's zero-filled BGRA allocation, but
            // do not create a swscale context, scale pixels, or publish to Rune.
            DiagnosticMode::AllocOnly => {
                let bytes = ((*frame).width as usize)
                    .saturating_mul((*frame).height as usize)
                    .saturating_mul(4);
                std::hint::black_box(vec![0_u8; bytes]);
                clock.observe_diagnostic_frame(frame);
                None
            }
            // Exercise sws_getContext, the BGRA allocation and sws_scale; drop
            // the converted frame before it can reach the Rune upload path.
            DiagnosticMode::ScaleOnly => {
                let converted = renderer.convert(frame, timestamp)?;
                std::hint::black_box(converted);
                clock.observe_diagnostic_frame(frame);
                None
            }
            // Same as scale-only, but SoftwareRenderer places a canary after
            // the output allocation and reports a destination overrun before
            // malloc has to detect it later.
            DiagnosticMode::ScaleGuard => {
                let converted = renderer.convert(frame, timestamp)?;
                std::hint::black_box(converted);
                clock.observe_diagnostic_frame(frame);
                None
            }
            // This is intentionally the ordinary renderer path, named so a
            // capture can explicitly include the Rune presentation boundary.
            DiagnosticMode::RenderOnly | DiagnosticMode::Disabled => {
                renderer.convert(frame, timestamp)?
            }
        };
        let processing_ms = processing_started.elapsed().as_secs_f64() * 1000.0;
        if let Some(frame) = rendered {
            publish_frame(frame, output, snapshot);
        }
        let mut state = snapshot.lock().unwrap();
        state.video_frames_processed = state.video_frames_processed.saturating_add(1);
        state.frame_processing_ms = Some(processing_ms);
        state.frame_processing_peak_ms = Some(
            state
                .frame_processing_peak_ms
                .unwrap_or(0.0)
                .max(processing_ms),
        );
        let count = state.video_frames_processed as f64;
        let previous = state.frame_processing_avg_ms.unwrap_or(processing_ms);
        state.frame_processing_avg_ms = Some(previous + (processing_ms - previous) / count);
    }
    Ok(())
}

pub(crate) fn publish_completed(
    renderer: &mut VideoRenderer,
    output: &Arc<FrameMailbox>,
    snapshot: &Arc<Mutex<super::super::state::Snapshot>>,
) -> Result<(), String> {
    if let Some(frame) = renderer.poll_completed()? {
        publish_frame(frame, output, snapshot);
    }
    Ok(())
}

pub(crate) fn finish_pending(
    renderer: &mut VideoRenderer,
    output: &Arc<FrameMailbox>,
    snapshot: &Arc<Mutex<super::super::state::Snapshot>>,
) -> Result<(), String> {
    if let Some(frame) = renderer.finish_pending()? {
        publish_frame(frame, output, snapshot);
    }
    Ok(())
}

fn publish_frame(
    frame: VideoFrame,
    output: &Arc<FrameMailbox>,
    snapshot: &Arc<Mutex<super::super::state::Snapshot>>,
) {
    snapshot.lock().unwrap().position = frame.timestamp();
    output.publish(frame);
}

unsafe fn timestamp_seconds(frame: *mut ffi::AVFrame, fallback: ffi::AVRational) -> f64 {
    let pts = if (*frame).best_effort_timestamp != i64::MIN {
        (*frame).best_effort_timestamp
    } else {
        (*frame).pts
    };
    let time_base = if (*frame).time_base.num > 0 && (*frame).time_base.den > 0 {
        (*frame).time_base
    } else {
        fallback
    };
    if pts == i64::MIN || time_base.num <= 0 || time_base.den <= 0 {
        0.0
    } else {
        pts as f64 * time_base.num as f64 / time_base.den as f64
    }
}

fn check(code: i32, operation: &str) -> Result<(), String> {
    if code < 0 {
        Err(format!("{operation} failed ({code})"))
    } else {
        Ok(())
    }
}
