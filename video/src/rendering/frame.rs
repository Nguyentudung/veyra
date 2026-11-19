#[derive(Clone, Debug)]
pub struct VideoFrame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub timestamp: f64,
}
