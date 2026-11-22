//! Latest-wins hand-off from the media worker to the UI thread.
//!
//! The mailbox intentionally holds one frame: showing a stale frame is worse
//! than dropping it, and retaining a queue of high-resolution frames would
//! inflate memory. Notifications are coalesced so a producer cannot flood the
//! window event loop while the UI is already scheduled to consume a frame.

use crate::rendering::VideoFrame;
use std::sync::{Arc, Mutex};

pub type FrameWaker = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
pub struct FrameMailbox {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    frame: Option<VideoFrame>,
    waker: Option<FrameWaker>,
    notification_pending: bool,
}

impl FrameMailbox {
    /// Replaces an unread frame and schedules one UI update, if needed.
    pub fn publish(&self, frame: VideoFrame) {
        let (old, wake) = {
            let mut state = self.state.lock().unwrap();
            let old = state.frame.replace(frame);
            let wake = Self::schedule_locked(&mut state);
            (old, wake)
        };
        // GPU leases and large CPU buffers may be released here. Do not hold
        // the mailbox lock while that happens.
        drop(old);
        if let Some(wake) = wake {
            wake();
        }
    }

    /// Takes the newest frame and permits the producer to schedule the next
    /// wake. The lock makes this transition race-free with `publish`.
    pub fn take_latest(&self) -> Option<VideoFrame> {
        let mut state = self.state.lock().unwrap();
        state.notification_pending = false;
        state.frame.take()
    }

    /// Drops an unread frame, such as when changing streams or seeking.
    pub fn clear(&self) {
        let old = self.state.lock().unwrap().frame.take();
        drop(old);
    }

    /// Installs the UI-thread callback. If playback changed during startup,
    /// request the first poll now rather than waiting for the status timer.
    pub fn set_waker(&self, waker: FrameWaker) {
        let (old, wake) = {
            let mut state = self.state.lock().unwrap();
            let had_waker = state.waker.is_some();
            let old = state.waker.replace(waker);
            let wake = (!had_waker && state.notification_pending)
                .then(|| state.waker.clone())
                .flatten();
            (old, wake)
        };
        drop(old);
        if let Some(wake) = wake {
            wake();
        }
    }

    /// Schedules an update for a playback-state change with no video frame,
    /// for example pause, seek, end-of-stream, volume, or an open error.
    pub fn wake(&self) {
        let wake = {
            let mut state = self.state.lock().unwrap();
            Self::schedule_locked(&mut state)
        };
        if let Some(wake) = wake {
            wake();
        }
    }

    fn schedule_locked(state: &mut State) -> Option<FrameWaker> {
        if state.notification_pending {
            None
        } else {
            state.notification_pending = true;
            state.waker.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn frame(timestamp: f64) -> VideoFrame {
        VideoFrame::Bytes {
            rgba: vec![0; 4],
            width: 1,
            height: 1,
            timestamp,
        }
    }

    #[test]
    fn coalesces_wakes_until_the_latest_frame_is_taken() {
        let mailbox = FrameMailbox::default();
        let wakes = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&wakes);
        mailbox.set_waker(Arc::new(move || {
            count.fetch_add(1, Ordering::Relaxed);
        }));

        mailbox.publish(frame(1.0));
        mailbox.publish(frame(2.0));
        assert_eq!(wakes.load(Ordering::Relaxed), 1);
        assert_eq!(mailbox.take_latest().unwrap().timestamp(), 2.0);

        mailbox.publish(frame(3.0));
        assert_eq!(wakes.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn delivers_a_startup_notification_when_the_waker_is_installed() {
        let mailbox = FrameMailbox::default();
        mailbox.wake();

        let wakes = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&wakes);
        mailbox.set_waker(Arc::new(move || {
            count.fetch_add(1, Ordering::Relaxed);
        }));

        assert_eq!(wakes.load(Ordering::Relaxed), 1);
    }
}
