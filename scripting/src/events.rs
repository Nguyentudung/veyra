use veyra_statistics::StatisticsSnapshot;

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum PlaybackEvent {
    Started,
    Paused,
    Resumed,
    Stopped,
    Finished,
    Seeked { position: f64 },
    SpeedChanged { speed: f64 },
    TrackChanged { file_name: Option<String> },
}

impl PlaybackEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Started => "playback_started",
            Self::Paused => "playback_paused",
            Self::Resumed => "playback_resumed",
            Self::Stopped => "playback_stopped",
            Self::Finished => "playback_finished",
            Self::Seeked { .. } => "playback_seeked",
            Self::SpeedChanged { .. } => "playback_speed_changed",
            Self::TrackChanged { .. } => "track_changed",
        }
    }
    pub fn payload(&self, stats: &StatisticsSnapshot) -> EventPayload {
        let position = match self {
            Self::Seeked { position } => *position,
            _ => stats.position,
        };
        let file_name = match self {
            Self::TrackChanged { file_name } => file_name.clone(),
            _ => stats.file_name.clone(),
        };
        EventPayload {
            event: self.name().into(),
            position,
            file_name,
        }
    }
}

#[derive(Clone, Debug)]
pub struct EventPayload {
    pub event: String,
    pub position: f64,
    pub file_name: Option<String>,
}
