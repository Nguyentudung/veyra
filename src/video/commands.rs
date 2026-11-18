use std::path::PathBuf;

pub(crate) enum Command {
    Open(PathBuf),
    Pause,
    Seek(f64),
    SeekTo(f64),
    Volume(f64),
    Mute,
    Stop,
}
