use std::collections::BTreeMap;

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub enum Metric {
    Number(f64),
    Missing,
}

#[cfg(test)]
mod tests {
    use super::{Metric, StatisticsSnapshot};
    #[test]
    fn missing_metrics_are_not_zero() {
        assert_eq!(
            StatisticsSnapshot::default().metric("video_fps"),
            Metric::Missing
        );
    }
}

#[derive(Clone, Debug, Default)]
pub struct StatisticsSnapshot {
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub container: Option<String>,
    pub file_size: Option<u64>,
    pub position: f64,
    pub duration: Option<f64>,
    pub playback_speed: f64,
    pub playback_state: String,
    pub video_codec: Option<String>,
    pub video_width: Option<u32>,
    pub video_height: Option<u32>,
    pub video_fps: Option<f64>,
    pub video_bitrate: Option<u64>,
    pub video_profile: Option<String>,
    pub video_pixel_format: Option<String>,
    pub video_decoder: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_sample_rate: Option<u32>,
    pub audio_channels: Option<u32>,
    pub audio_bitrate: Option<u64>,
    pub audio_output: Option<String>,
    pub video_frames_processed: u64,
    pub frame_processing_ms: Option<f64>,
    pub frame_processing_avg_ms: Option<f64>,
    pub frame_processing_peak_ms: Option<f64>,
    pub audio_processing_ms: Option<f64>,
    pub dropped_frames: Option<u64>,
    pub history: Vec<f64>,
    pub frame_history: Vec<f64>,
    pub averages: BTreeMap<String, f64>,
}

#[allow(dead_code)]
impl StatisticsSnapshot {
    pub fn metric(&self, name: &str) -> Metric {
        match name {
            "position" => Metric::Number(self.position),
            "duration" => self.duration.map_or(Metric::Missing, Metric::Number),
            "playback_speed" => Metric::Number(self.playback_speed),
            "video_width" => self
                .video_width
                .map_or(Metric::Missing, |v| Metric::Number(v as f64)),
            "video_height" => self
                .video_height
                .map_or(Metric::Missing, |v| Metric::Number(v as f64)),
            "video_fps" => self.video_fps.map_or(Metric::Missing, Metric::Number),
            "frame_processing_ms" => self
                .frame_processing_ms
                .map_or(Metric::Missing, Metric::Number),
            "dropped_frames" => self
                .dropped_frames
                .map_or(Metric::Missing, |v| Metric::Number(v as f64)),
            _ => Metric::Missing,
        }
    }
}
