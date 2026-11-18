use super::super::ffi;
use super::{frame::VideoFrame, renderer::SoftwareRenderer};
use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

pub(crate) struct PresentationClock {
    started_at: Option<Instant>,
    first_pts: Option<f64>,
}

impl PresentationClock {
    pub(crate) fn new() -> Self {
        Self {
            started_at: None,
            first_pts: None,
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
}

pub(crate) unsafe fn decode_video(
    context: *mut ffi::AVCodecContext,
    packet: *mut ffi::AVPacket,
    frame: *mut ffi::AVFrame,
    renderer: &mut SoftwareRenderer,
    output: &Arc<Mutex<Option<VideoFrame>>>,
    snapshot: &Arc<Mutex<super::super::state::Snapshot>>,
    time_base: ffi::AVRational,
    clock: &mut PresentationClock,
) -> Result<(), String> {
    check(
        ffi::avcodec_send_packet(context, packet),
        "send video packet",
    )?;
    while ffi::avcodec_receive_frame(context, frame) >= 0 {
        let processing_started = Instant::now();
        let (rgba, width, height) = renderer.convert(frame)?;
        let processing_ms = processing_started.elapsed().as_secs_f64() * 1000.0;
        let timestamp = timestamp_seconds(frame, time_base);
        clock.wait_until(timestamp);
        *output.lock().unwrap() = Some(VideoFrame {
            rgba,
            width,
            height,
        });
        let mut state = snapshot.lock().unwrap();
        state.position = timestamp;
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
