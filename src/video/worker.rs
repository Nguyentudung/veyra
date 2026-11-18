use super::{commands::Command, decoder::Decoder, rendering::VideoFrame, state::Snapshot};
use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

pub(crate) fn run(
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
                            decoder = Some(new_decoder);
                            paused = false;
                            let mut state = snapshot.lock().unwrap();
                            state.paused = false;
                            state.error = None;
                            state.position = 0.0;
                            state.duration = duration;
                        }
                        Err(error) => snapshot.lock().unwrap().error = Some(error),
                    }
                }
                Command::Pause => {
                    paused = !paused;
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
                snapshot.lock().unwrap().error = Some(error);
                decoder = None;
            }
        } else {
            thread::sleep(Duration::from_millis(16));
        }
    }
}
