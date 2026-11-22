//! Latest-wins hand-off from the media worker to the UI thread.
//!
//! The mailbox intentionally holds one frame: showing a stale frame is worse
//! than dropping it, and retaining a queue of high-resolution frames would
//! inflate memory. Notifications are coalesced so a producer cannot flood the
//! window event loop while the UI is already scheduled to consume a frame.

use crate::rendering::VideoFrame;
use std::sync::{Arc, Mutex};

pub type FrameWaker = Arc<dyn Fn() + Send + Sync>;

/// One atomic delivery from the media worker to the UI thread.
///
/// A frame is source-paced and can be presented without rebuilding playback
/// controls. State-only changes, such as pause or seek, require a fresh
/// snapshot immediately.
pub struct FrameUpdate {
    pub frame: Option<VideoFrame>,
    pub state_changed: bool,
}

#[derive(Default)]
pub struct FrameMailbox {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    frame: Option<VideoFrame>,
    waker: Option<FrameWaker>,
    notification_pending: bool,
    state_changed: bool,
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

    /// Atomically takes the newest frame and its state-change marker.
    ///
    /// This permits the producer to schedule the next wake. Keeping both
    /// values under the same lock ensures a state-only wake cannot be lost
    /// between a frame read and a snapshot refresh decision.
    pub fn take_update(&self) -> FrameUpdate {
        let mut state = self.state.lock().unwrap();
        state.notification_pending = false;
        FrameUpdate {
            frame: state.frame.take(),
            state_changed: std::mem::take(&mut state.state_changed),
        }
    }

    /// Takes the newest frame, leaving the worker with a bounded one-frame
    /// slot. Callers that also need state freshness should use [`take_update`].
    pub fn take_latest(&self) -> Option<VideoFrame> {
        self.take_update().frame
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
            state.state_changed = true;
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

    #[test]
    fn reports_state_changes_with_the_frame_atomically() {
        let mailbox = FrameMailbox::default();
        mailbox.publish(frame(1.0));
        mailbox.wake();

        let update = mailbox.take_update();
        assert_eq!(update.frame.unwrap().timestamp(), 1.0);
        assert!(update.state_changed);

        let next = mailbox.take_update();
        assert!(next.frame.is_none());
        assert!(!next.state_changed);
    }
}
