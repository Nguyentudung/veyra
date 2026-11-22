//! Media implementation: FFmpeg decoding, audio output, video rendering and worker state.

#![allow(unsafe_op_in_unsafe_fn)]

pub(crate) mod audio;
mod commands;
pub(crate) mod decoder;
pub(crate) mod ffi;
mod mailbox;
pub(crate) mod rendering;
mod state;
pub(crate) mod worker;

pub use commands::Command;
pub use mailbox::{FrameMailbox, FrameUpdate, FrameWaker};
pub use rendering::VideoFrame;
pub use rune::native_surface::GpuContext;
pub use state::Snapshot;
pub use worker::run;
