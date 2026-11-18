use super::{commands::Command, decoder::Decoder, rendering::VideoFrame, state::Snapshot};
use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

pub fn run(
    rx: mpsc::Receiver<Command>,
    snapshot: Arc<Mutex<Snapshot>>,
    frame: Arc<Mutex<Option<VideoFrame>>>,
) {
    let mut decoder = None;
    let mut paused = false;
    loop {
        while let Ok(command) = rx.try_recv() {
            match command {
                Command::Open(path) => {
                    decoder = None;
                    match Decoder::open(&path) {
                        Ok(new_decoder) => {
                            let duration = new_decoder.duration();
                            let metadata = new_decoder.metadata();
                            decoder = Some(new_decoder);
                            paused = false;
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
                        }
                        Err(error) => snapshot.lock().unwrap().error = Some(error),
                    }
                }
                Command::Pause => {
                    paused = !paused;
                    if let Some(decoder) = decoder.as_mut() {
                        if paused {
                            decoder.pause_audio();
                        } else {
                            decoder.resume_audio();
                            decoder.reset_presentation_clock();
                        }
                    }
                    snapshot.lock().unwrap().paused = paused;
                }
                Command::Seek(delta) => {
                    if let Some(decoder) = decoder.as_mut() {
                        *frame.lock().unwrap() = None;
                        if let Err(error) = decoder.seek(delta, &snapshot) {
                            snapshot.lock().unwrap().error = Some(error);
                        }
                    }
                }
                Command::SeekTo(target) => {
                    if let Some(decoder) = decoder.as_mut() {
                        if let Err(error) = decoder.seek_to(target, &snapshot, &frame) {
                            snapshot.lock().unwrap().error = Some(error);
                        }
                    }
                }
                Command::Volume(volume) => snapshot.lock().unwrap().volume = volume,
                Command::Mute => {
                    let mut state = snapshot.lock().unwrap();
                    state.muted = !state.muted;
                }
                Command::Stop => return,
            }
        }
        if paused {
            thread::sleep(Duration::from_millis(8));
        } else if decoder.is_some() {
            let result = decoder
                .as_mut()
                .expect("decoder exists")
                .step(&snapshot, &frame);
            if let Err(error) = result {
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
                decoder = None;
            }
        } else {
            thread::sleep(Duration::from_millis(16));
        }
    }
}
