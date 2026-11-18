use super::{aggregator::Aggregator, history::History, model::StatisticsSnapshot};
use crate::playback::Snapshot;
use std::{path::Path, time::Instant};

pub struct StatisticsCollector {
    history: History,
    frame_history: History,
    aggregator: Aggregator,
    last: Instant,
    last_render: Option<Instant>,
    render_window: Instant,
    rendered_frames: u64,
    render_fps: Option<f64>,
    frame_interval_ms: Option<f64>,
    frame_jitter_ms: Option<f64>,
    previous_frame_interval_ms: Option<f64>,
    total_rendered_frames: u64,
}

impl StatisticsCollector {
    pub fn new() -> Self {
        Self {
            history: History::new(120),
            frame_history: History::new(120),
            aggregator: Aggregator::default(),
            last: Instant::now(),
            last_render: None,
            render_window: Instant::now(),
            rendered_frames: 0,
            render_fps: None,
            frame_interval_ms: None,
            frame_jitter_ms: None,
            previous_frame_interval_ms: None,
            total_rendered_frames: 0,
        }
    }
    pub fn record_rendered_frame(&mut self) {
        let now = Instant::now();
        if let Some(previous) = self.last_render {
            let interval = now.duration_since(previous).as_secs_f64() * 1000.0;
            self.frame_interval_ms = Some(interval);
            self.frame_jitter_ms = self
                .previous_frame_interval_ms
                .map(|previous| (interval - previous).abs());
            self.previous_frame_interval_ms = Some(interval);
        }
        self.last_render = Some(now);
        self.rendered_frames = self.rendered_frames.saturating_add(1);
        self.total_rendered_frames = self.total_rendered_frames.saturating_add(1);
        let elapsed = now.duration_since(self.render_window).as_secs_f64();
        if elapsed >= 0.5 {
            self.render_fps = Some(self.rendered_frames as f64 / elapsed);
            self.rendered_frames = 0;
            self.render_window = now;
        }
    }
    pub fn reset_stream(&mut self) {
        self.last_render = None;
        self.render_window = Instant::now();
        self.rendered_frames = 0;
        self.render_fps = None;
        self.frame_interval_ms = None;
        self.frame_jitter_ms = None;
        self.previous_frame_interval_ms = None;
        self.total_rendered_frames = 0;
    }
    pub fn sample(&mut self, snapshot: &Snapshot, path: Option<&Path>) -> StatisticsSnapshot {
        if self.last.elapsed().as_millis() >= 250 {
            self.history.push(snapshot.position);
            if let Some(frame_ms) = snapshot.frame_processing_ms {
                self.frame_history.push(frame_ms);
            }
            self.last = Instant::now();
        }
        self.aggregator.add("position", snapshot.position);
        StatisticsSnapshot {
            file_name: snapshot.file_name.clone().or_else(|| {
                path.and_then(|p| p.file_name())
                    .and_then(|p| p.to_str())
                    .map(str::to_owned)
            }),
            file_path: snapshot.file_path.clone(),
            container: snapshot.container.clone(),
            file_size: snapshot.file_size,
            position: snapshot.position,
            duration: snapshot.duration,
            playback_speed: 1.0,
            playback_state: if snapshot.finished {
                "finished"
            } else if snapshot.error.is_some() {
                "error"
            } else if snapshot.file_name.is_none() {
                "idle"
            } else if snapshot.paused {
                "paused"
            } else {
                "playing"
            }
            .into(),
            video_codec: snapshot.video_codec.clone(),
            video_width: (snapshot.video_width > 0).then_some(snapshot.video_width),
            video_height: (snapshot.video_height > 0).then_some(snapshot.video_height),
            video_fps: snapshot.video_fps,
            video_bitrate: snapshot.video_bitrate,
            video_profile: snapshot.video_profile.clone(),
            video_pixel_format: snapshot.video_pixel_format.clone(),
            video_decoder: snapshot.video_decoder.clone(),
            audio_codec: snapshot.audio_codec.clone(),
            audio_sample_rate: snapshot.audio_sample_rate,
            audio_channels: snapshot.audio_channels,
            audio_bitrate: snapshot.audio_bitrate,
            audio_output: snapshot.audio_output.clone(),
            video_frames_processed: snapshot.video_frames_processed,
            rendered_frames: self.total_rendered_frames,
            render_fps: self.render_fps,
            frame_interval_ms: self.frame_interval_ms,
            frame_jitter_ms: self.frame_jitter_ms,
            frame_processing_ms: snapshot.frame_processing_ms,
            frame_processing_avg_ms: snapshot.frame_processing_avg_ms,
            frame_processing_peak_ms: snapshot.frame_processing_peak_ms,
            audio_processing_ms: snapshot.audio_processing_ms,
            dropped_frames: (snapshot.video_frames_processed >= self.total_rendered_frames)
                .then_some(snapshot.video_frames_processed - self.total_rendered_frames),
            history: self.history.values(),
            frame_history: self.frame_history.values(),
            averages: self.aggregator.averages(),
        }
    }
}
