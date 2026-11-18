use super::super::ffi;
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};
use veyra_cache::{MemoryBudget, Reservation};

pub(super) const OUTPUT_RATE: i64 = 48_000;
const OUTPUT_CHANNELS: u32 = 2;
const MAX_QUEUED_FRAMES: u64 = OUTPUT_RATE as u64 * 2;
const PREBUFFER_FRAMES: u64 = OUTPUT_RATE as u64 / 5;

struct AudioChunk {
    samples: Vec<u8>,
    frames: u64,
    _reservation: Reservation,
}

struct QueueState {
    chunks: VecDeque<AudioChunk>,
    queued_frames: u64,
    paused: bool,
    stopping: bool,
    started: bool,
}

struct AudioQueue {
    state: Mutex<QueueState>,
    wake: Condvar,
    budget: MemoryBudget,
}

pub(crate) struct AudioOutput {
    pub(super) swr: *mut ffi::SwrContext,
    queue: Arc<AudioQueue>,
    thread: Option<JoinHandle<()>>,
}

impl AudioOutput {
    pub(super) unsafe fn open(
        rate: i32,
        channels: u32,
        sample_format: i32,
    ) -> Result<Self, String> {
        let mut pcm = std::ptr::null_mut();
        let name = std::ffi::CString::new("default").unwrap();
        check(
            ffi::snd_pcm_open(&mut pcm, name.as_ptr(), 0, 0),
            "open ALSA output",
        )?;
        check(
            ffi::snd_pcm_set_params(
                pcm,
                ffi::_snd_pcm_format_SND_PCM_FORMAT_S16_LE,
                ffi::_snd_pcm_access_SND_PCM_ACCESS_RW_INTERLEAVED,
                OUTPUT_CHANNELS,
                OUTPUT_RATE as u32,
                1,
                500_000,
            ),
            "configure ALSA output",
        )?;
        let swr = super::resampler::create(sample_format, rate, channels)?;
        let queue = Arc::new(AudioQueue {
            state: Mutex::new(QueueState {
                chunks: VecDeque::new(),
                queued_frames: 0,
                paused: false,
                stopping: false,
                started: false,
            }),
            wake: Condvar::new(),
            budget: MemoryBudget::new(8 * 1024 * 1024),
        });
        let queue_for_thread = Arc::clone(&queue);
        let pcm_address = pcm as usize;
        let thread = thread::spawn(move || unsafe {
            run_audio_output(pcm_address as *mut ffi::snd_pcm_t, queue_for_thread);
        });
        eprintln!(
            "veyra audio: ALSA default, input {} Hz/{} channels/format {}, output {} Hz/{} channels S16_LE",
            rate, channels, sample_format, OUTPUT_RATE, OUTPUT_CHANNELS
        );
        Ok(Self {
            swr,
            queue,
            thread: Some(thread),
        })
    }

    pub(crate) fn pause(&mut self) {
        let mut state = self.queue.state.lock().unwrap();
        state.paused = true;
        state.started = false;
        state.chunks.clear();
        state.queued_frames = 0;
        self.queue.wake.notify_all();
    }

    pub(crate) fn resume(&mut self) {
        let mut state = self.queue.state.lock().unwrap();
        state.paused = false;
        self.queue.wake.notify_all();
    }

    pub(super) fn write(&mut self, samples: &[u8], frames: u64) -> Result<(), String> {
        if frames == 0 {
            return Ok(());
        }
        let reservation = loop {
            let mut state = self.queue.state.lock().unwrap();
            while state.queued_frames + frames > MAX_QUEUED_FRAMES && !state.stopping {
                state = self.queue.wake.wait(state).unwrap();
            }
            if state.stopping {
                return Err("audio output stopped".into());
            }
            drop(state);
            if let Some(reservation) = self.queue.budget.try_reserve(samples.len()) {
                break reservation;
            }
            let state = self.queue.state.lock().unwrap();
            drop(self.queue.wake.wait(state).unwrap());
        };
        let mut state = self.queue.state.lock().unwrap();
        if state.stopping {
            return Err("audio output stopped".into());
        }
        state.chunks.push_back(AudioChunk {
            samples: samples.to_vec(),
            frames,
            _reservation: reservation,
        });
        state.queued_frames += frames;
        self.queue.wake.notify_one();
        Ok(())
    }
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        {
            let mut state = self.queue.state.lock().unwrap();
            state.stopping = true;
            state.chunks.clear();
            state.queued_frames = 0;
            self.queue.wake.notify_all();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        unsafe {
            let mut swr = self.swr;
            ffi::swr_free(&mut swr);
        }
    }
}

unsafe fn run_audio_output(pcm: *mut ffi::snd_pcm_t, queue: Arc<AudioQueue>) {
    let bytes_per_frame = (OUTPUT_CHANNELS as usize) * 2;
    loop {
        let chunk = {
            let mut state = queue.state.lock().unwrap();
            if state.paused && !state.stopping {
                drop(state);
                // This also handles a pause that arrives just after the last
                // queued chunk was written and there is no active write left
                // to perform the flush below.
                ffi::snd_pcm_drop(pcm);
                let _ = ffi::snd_pcm_prepare(pcm);
                state = queue.state.lock().unwrap();
            }
            while (state.paused
                || state.chunks.is_empty()
                || (!state.started && state.queued_frames < PREBUFFER_FRAMES))
                && !state.stopping
            {
                state = queue.wake.wait(state).unwrap();
            }
            if state.stopping {
                break;
            }
            state.started = true;
            let chunk = state.chunks.pop_front().expect("audio chunk exists");
            state.queued_frames = state.queued_frames.saturating_sub(chunk.frames);
            queue.wake.notify_all();
            chunk
        };

        let mut offset = 0u64;
        while offset < chunk.frames {
            let paused_or_stopping = {
                let state = queue.state.lock().unwrap();
                state.paused || state.stopping
            };
            if paused_or_stopping {
                break;
            }
            let pointer = chunk
                .samples
                .as_ptr()
                .add(offset as usize * bytes_per_frame)
                .cast();
            let written = ffi::snd_pcm_writei(pcm, pointer, chunk.frames - offset);
            if written < 0 {
                let recovered = ffi::snd_pcm_recover(pcm, written as i32, 1);
                if recovered < 0 {
                    eprintln!(
                        "veyra audio: output write failed ({written}), recovery failed ({recovered})"
                    );
                    break;
                }
                continue;
            }
            if written == 0 {
                eprintln!("veyra audio: output write made no progress");
                break;
            }
            offset += written as u64;
        }

        let state = queue.state.lock().unwrap();
        if state.paused {
            // Flush stale samples after pause so they cannot play ahead of the
            // reset video clock when playback resumes.
            ffi::snd_pcm_drop(pcm);
            let _ = ffi::snd_pcm_prepare(pcm);
        }
        if state.stopping {
            break;
        }
    }
    ffi::snd_pcm_drop(pcm);
    ffi::snd_pcm_close(pcm);
}

fn check(code: i32, operation: &str) -> Result<(), String> {
    if code < 0 {
        Err(format!("{operation} failed ({code})"))
    } else {
        Ok(())
    }
}
