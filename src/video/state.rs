#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub paused: bool,
    pub position: f64,
    pub duration: Option<f64>,
    pub volume: f64,
    pub muted: bool,
    pub error: Option<String>,
    pub video_width: u32,
    pub video_height: u32,
}
