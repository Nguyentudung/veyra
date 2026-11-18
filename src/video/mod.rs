//! Media implementation: FFmpeg decoding, audio output, video rendering and worker state.

#![allow(unsafe_op_in_unsafe_fn)]

pub(crate) mod audio;
pub(crate) mod commands;
pub(crate) mod decoder;
pub(crate) mod ffi;
pub(crate) mod rendering;
pub(crate) mod state;
pub(crate) mod worker;

pub(crate) use rendering::VideoFrame;
