use super::{FrameMailbox, commands::Command, decoder::Decoder, state::Snapshot};
use std::sync::{Arc, Mutex, mpsc};

pub fn run(rx: mpsc::Receiver<Command>, snapshot: Arc<Mutex<Snapshot>>, frame: Arc<FrameMailbox>) {
    let mut decoder = None;
    // Rune creates its device lazily. Keep the most recent published context
    // so opening a file before (or after) the first UI frame has identical
    // native-renderer behavior.
    let mut gpu_context = None;
    let mut paused = false;
    loop {
        // There is no work to do while paused or without an open decoder.
        // Blocking here replaces the old 8/16 ms polling loops and lets the
        // worker consume no CPU or wakeups in those states.
        if paused || decoder.is_none() {
            let command = match rx.recv() {
                Ok(command) => command,
                Err(_) => return,
            };
            if handle_command(
                command,
                &mut decoder,
                &mut gpu_context,
                &mut paused,
                &snapshot,
                &frame,
            ) {
                return;
            }
            continue;
        }

        loop {
            let command = match rx.try_recv() {
                Ok(command) => command,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            };
            if handle_command(
                command,
                &mut decoder,
                &mut gpu_context,
                &mut paused,
                &snapshot,
                &frame,
            ) {
                return;
            }
        }
        if paused || decoder.is_none() {
            continue;
        }
        let result = decoder
            .as_mut()
            .expect("decoder exists while playback is active")
            .step(&snapshot, &frame);
        if let Err(error) = result {
            {
                let mut state = snapshot.lock().unwrap();
                if error == "playback finished" {
                    state.finished = true;
                    state.paused = true;
                    if let Some(duration) = state.duration {
                        state.position = duration;
                    }
                } else {
                    state.error = Some(error);
                }
            }
            decoder = None;
            frame.wake();
        }
    }
}

fn handle_command(
    command: Command,
    decoder: &mut Option<Decoder>,
    gpu_context: &mut Option<crate::GpuContext>,
    paused: &mut bool,
    snapshot: &Arc<Mutex<Snapshot>>,
    frame: &Arc<FrameMailbox>,
) -> bool {
    match command {
        Command::Open(path) => {
            // Release the old latest-frame allocation/lease before opening
            // another decoder. Otherwise a high-resolution frame can remain
            // retained until the next UI poll.
            frame.clear();
            *decoder = None;
            match Decoder::open(&path, gpu_context.as_ref()) {
                Ok(new_decoder) => {
                    let duration = new_decoder.duration();
                    let metadata = new_decoder.metadata();
                    *decoder = Some(new_decoder);
                    *paused = false;
                    let mut state = snapshot.lock().unwrap();
                    state.paused = false;
                    state.finished = false;
                    state.error = None;
                    state.position = 0.0;
                    state.duration = duration;
                    state.file_name = metadata.file_name;
                    state.file_path = metadata.file_path;
                    state.container = metadata.container;
                    state.file_size = metadata.file_size;
                    state.video_codec = metadata.video_codec;
                    state.audio_codec = metadata.audio_codec;
                    state.audio_sample_rate = metadata.audio_sample_rate;
                    state.audio_channels = metadata.audio_channels;
                    state.video_fps = metadata.video_fps;
                    state.video_bitrate = metadata.video_bitrate;
                    state.audio_bitrate = metadata.audio_bitrate;
                    state.video_decoder = metadata.video_decoder;
                    state.audio_output = metadata.audio_output;
                    state.video_frames_processed = 0;
                    state.frame_processing_ms = None;
                    state.frame_processing_avg_ms = None;
                    state.frame_processing_peak_ms = None;
                    state.audio_processing_ms = None;
                    state.video_profile = metadata.video_profile;
                    state.video_pixel_format = metadata.video_pixel_format;
                    state.video_decode_threads = metadata.video_decode_threads;
                    state.video_decode_thread_type = metadata.video_decode_thread_type;
                }
                Err(error) => snapshot.lock().unwrap().error = Some(error),
            }
            frame.wake();
        }
        Command::GpuContext(context) => {
            *gpu_context = Some(context.clone());
            if let Some(decoder) = decoder.as_mut() {
                decoder.set_gpu_context(context);
            }
        }
        Command::Pause => {
            *paused = !*paused;
            if let Some(decoder) = decoder.as_mut() {
                if *paused {
                    decoder.pause_audio();
                } else {
                    decoder.resume_audio();
                    decoder.reset_presentation_clock();
                }
            }
            snapshot.lock().unwrap().paused = *paused;
            frame.wake();
        }
        Command::Seek(delta) => {
            if let Some(decoder) = decoder.as_mut() {
                frame.clear();
                if let Err(error) = decoder.seek(delta, snapshot) {
                    snapshot.lock().unwrap().error = Some(error);
                }
                frame.wake();
            }
        }
        Command::SeekTo(target) => {
            if let Some(decoder) = decoder.as_mut() {
                if let Err(error) = decoder.seek_to(target, snapshot, frame) {
                    snapshot.lock().unwrap().error = Some(error);
                }
                frame.wake();
            }
        }
        Command::Volume(volume) => {
            snapshot.lock().unwrap().volume = volume;
            frame.wake();
        }
        Command::Mute => {
            let mut state = snapshot.lock().unwrap();
            state.muted = !state.muted;
            drop(state);
            frame.wake();
        }
        Command::Stop => return true,
    }
    false
}
