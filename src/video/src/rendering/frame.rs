/// Latest decoded image in tightly packed BGRA format.
pub enum VideoFrame {
    Bytes {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        timestamp: f64,
    },
}

impl VideoFrame {
    pub fn timestamp(&self) -> f64 {
        match self {
            Self::Bytes { timestamp, .. } => *timestamp,
        }
    }
}
