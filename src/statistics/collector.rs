use super::{aggregator::Aggregator, history::History, model::StatisticsSnapshot};
use crate::playback::Snapshot;
use std::{path::Path, time::Instant};

pub struct StatisticsCollector {
    history: History,
    frame_history: History,
    aggregator: Aggregator,
    last: Instant,
}

impl StatisticsCollector {
    pub fn new() -> Self {
        Self {
            history: History::new(120),
            frame_history: History::new(120),
            aggregator: Aggregator::default(),
            last: Instant::now(),
        }
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
            playback_state: if snapshot.error.is_some() {
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
            frame_processing_ms: snapshot.frame_processing_ms,
            frame_processing_avg_ms: snapshot.frame_processing_avg_ms,
            frame_processing_peak_ms: snapshot.frame_processing_peak_ms,
            audio_processing_ms: snapshot.audio_processing_ms,
            dropped_frames: None,
            history: self.history.values(),
            frame_history: self.frame_history.values(),
            averages: self.aggregator.averages(),
        }
    }
}
