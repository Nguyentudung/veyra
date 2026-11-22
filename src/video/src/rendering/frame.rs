use rune::native_surface::{GpuFrame, GpuVideoFrame};

/// The media worker has one latest-wins slot, so a frame can either carry CPU
/// BGRA bytes for the portable route or a texture rendered on Rune's queue.
///
/// `GpuFrame` deliberately is not cloneable: its producer lease is the
/// ownership proof that an output-pool slot cannot be reused while Rune still
/// samples it.
pub enum VideoFrame {
    Bytes {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        timestamp: f64,
    },
    Gpu {
        frame: GpuFrame,
        timestamp: f64,
    },
    GpuVideo {
        frame: GpuVideoFrame,
        timestamp: f64,
    },
}

impl VideoFrame {
    pub fn timestamp(&self) -> f64 {
        match self {
            Self::Bytes { timestamp, .. }
            | Self::Gpu { timestamp, .. }
            | Self::GpuVideo { timestamp, .. } => *timestamp,
        }
    }
}
