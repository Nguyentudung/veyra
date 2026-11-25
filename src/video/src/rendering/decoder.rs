use super::super::ffi;
use super::{frame::VideoFrame, renderer::VideoRenderer};
use crate::FrameMailbox;
use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

// Sleeping until a presentation deadline can overshoot by around a scheduler
// tick. Spend a deliberately small CPU tail to land source-paced frames more
// consistently, without busy-waiting through an entire frame interval.
const PRESENTATION_SPIN_TAIL: Duration = Duration::from_micros(250);

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
            if remaining > PRESENTATION_SPIN_TAIL {
                thread::sleep(remaining - PRESENTATION_SPIN_TAIL);
            }
            while Instant::now() < target {
                std::hint::spin_loop();
            }
        }
    }

    fn diagnostics(&self) -> DiagnosticMode {
        self.diagnostics
    }

    /// Reports diagnostic frames at a low rate without adding work per frame.
    fn observe_diagnostic_frame(&mut self, frame: *const ffi::AVFrame, timestamp: f64) {
        self.diagnostics_frames = self.diagnostics_frames.saturating_add(1);
        let elapsed = self.diagnostics_started.elapsed();
        if elapsed < Duration::from_secs(1) {
            return;
        }
        unsafe {
            eprintln!(
                "[video][{}] interval_ms={:.0} decoded_frames={} timestamp={timestamp:.3} best_effort_pts={} frame_pts={} frame_time_base={}/{} size={}x{} format={} strides=[{}, {}, {}]",
                self.diagnostics.label().unwrap_or("disabled"),
                elapsed.as_secs_f64() * 1000.0,
                self.diagnostics_frames,
                (*frame).best_effort_timestamp,
                (*frame).pts,
                (*frame).time_base.num,
                (*frame).time_base.den,
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
                clock.observe_diagnostic_frame(frame, timestamp);
                None
            }
            // Match the software renderer's BGRA allocation size, but do not
            // create a swscale context, scale pixels, or publish to Rune.
            DiagnosticMode::AllocOnly => {
                let bytes = ((*frame).width as usize)
                    .saturating_mul((*frame).height as usize)
                    .saturating_mul(4);
                std::hint::black_box(vec![0_u8; bytes]);
                clock.observe_diagnostic_frame(frame, timestamp);
                None
            }
            // Exercise sws_getContext, the BGRA allocation and sws_scale; drop
            // the converted frame before it can reach the Rune upload path.
            DiagnosticMode::ScaleOnly => {
                let converted = renderer.convert(frame, timestamp)?;
                std::hint::black_box(converted);
                clock.observe_diagnostic_frame(frame, timestamp);
                None
            }
            // Same as scale-only, but SoftwareRenderer places a canary after
            // the output allocation and reports a destination overrun before
            // malloc has to detect it later.
            DiagnosticMode::ScaleGuard => {
                let converted = renderer.convert(frame, timestamp)?;
                std::hint::black_box(converted);
                clock.observe_diagnostic_frame(frame, timestamp);
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
    timestamp_seconds_from_parts(
        (*frame).best_effort_timestamp,
        (*frame).pts,
        fallback,
        (*frame).time_base,
    )
}

/// `best_effort_timestamp` is explicitly expressed in the stream time base.
/// It must not be combined with `AVFrame::time_base`, which decoders and
/// filters may set to a different value. `AVFrame::pts`, on the other hand,
/// is expressed in the frame time base when one is supplied.
fn timestamp_seconds_from_parts(
    best_effort_timestamp: i64,
    pts: i64,
    stream_time_base: ffi::AVRational,
    frame_time_base: ffi::AVRational,
) -> f64 {
    if best_effort_timestamp != i64::MIN {
        return timestamp_in_time_base(best_effort_timestamp, stream_time_base);
    }
    let time_base = valid_time_base(frame_time_base)
        .then_some(frame_time_base)
        .unwrap_or(stream_time_base);
    timestamp_in_time_base(pts, time_base)
}

fn timestamp_in_time_base(timestamp: i64, time_base: ffi::AVRational) -> f64 {
    if timestamp == i64::MIN || time_base.num <= 0 || time_base.den <= 0 {
        0.0
    } else {
        timestamp as f64 * time_base.num as f64 / time_base.den as f64
    }
}

fn valid_time_base(time_base: ffi::AVRational) -> bool {
    time_base.num > 0 && time_base.den > 0
}

fn check(code: i32, operation: &str) -> Result<(), String> {
    if code < 0 {
        Err(format!("{operation} failed ({code})"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time_base(num: i32, den: i32) -> ffi::AVRational {
        ffi::AVRational { num, den }
    }

    #[test]
    fn best_effort_timestamp_uses_the_stream_time_base() {
        // A decoder/filter is allowed to set a frame time base that differs
        // from the stream. best_effort_timestamp remains in stream units.
        let seconds =
            timestamp_seconds_from_parts(1_000, 2, time_base(1, 60_000), time_base(1, 120));
        assert!((seconds - 1.0 / 60.0).abs() < 1e-12);
    }

    #[test]
    fn raw_pts_uses_the_frame_time_base_when_best_effort_is_missing() {
        let seconds =
            timestamp_seconds_from_parts(i64::MIN, 2, time_base(1, 60_000), time_base(1, 120));
        assert!((seconds - 1.0 / 60.0).abs() < 1e-12);
    }
}
