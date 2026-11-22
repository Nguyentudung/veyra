//! Playback engine façade and worker orchestration.

#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
};
use veyra_video::{Command, FrameMailbox, FrameWaker, GpuContext};
pub use veyra_video::{Snapshot, VideoFrame};

pub struct Engine {
    tx: mpsc::Sender<Command>,
    snapshot: Arc<Mutex<Snapshot>>,
    frame: Arc<FrameMailbox>,
    worker: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let snapshot = Arc::new(Mutex::new(Snapshot {
            volume: 100.0,
            ..Default::default()
        }));
        let frame = Arc::new(FrameMailbox::default());
        let worker_snapshot = Arc::clone(&snapshot);
        let worker_frame = Arc::clone(&frame);
        let worker = thread::Builder::new()
            .name("veyra-media-engine".into())
            .spawn(move || veyra_video::run(rx, worker_snapshot, worker_frame))
            .expect("media worker thread");
        Self {
            tx,
            snapshot,
            frame,
            worker: Some(worker),
        }
    }

    pub fn open(&self, path: &Path) -> Result<(), String> {
        if !path.is_file() {
            return Err(format!("File not found: {}", path.display()));
        }
        self.tx
            .send(Command::Open(path.to_path_buf()))
            .map_err(|e| e.to_string())
    }
    pub fn toggle_pause(&self) {
        let _ = self.tx.send(Command::Pause);
    }
    pub fn seek(&self, seconds: f64) {
        let _ = self.tx.send(Command::Seek(seconds));
    }
    pub fn seek_to(&self, seconds: f64) {
        let _ = self.tx.send(Command::SeekTo(seconds));
    }
    pub fn set_volume(&self, volume: f64) {
        let _ = self.tx.send(Command::Volume(volume));
    }
    pub fn toggle_mute(&self) {
        let _ = self.tx.send(Command::Mute);
    }
    /// Supplies Rune's shared device/queue to the media worker. The worker
    /// only consumes it when `VEYRA_VIDEO_RENDERER=gpu-native` is selected.
    pub fn set_gpu_context(&self, context: GpuContext) {
        let _ = self.tx.send(Command::GpuContext(context));
    }
    /// Makes the media worker wake the UI when a source-paced frame or a
    /// playback-state change is ready. The callback is coalesced by the
    /// one-frame mailbox so a fast decoder cannot flood the event loop.
    pub fn set_waker(&self, waker: FrameWaker) {
        self.frame.set_waker(waker);
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().unwrap().clone()
    }
    /// Takes the newest frame, leaving the worker with a bounded one-frame slot.
    pub fn take_latest_frame(&self) -> Option<VideoFrame> {
        self.frame.take_latest()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
