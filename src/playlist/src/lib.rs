use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
}

#[derive(Default, Debug)]
pub struct Playlist {
    tracks: Vec<Track>,
    current: Option<usize>,
}

impl Playlist {
    pub fn add(&mut self, path: impl Into<PathBuf>) -> usize {
        let path = path.into();
        let title = path
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("Untitled")
            .to_string();
        self.tracks.push(Track { path, title });
        let index = self.tracks.len() - 1;
        if self.current.is_none() {
            self.current = Some(index);
        }
        index
    }
    pub fn select(&mut self, index: usize) -> Option<&Track> {
        if index < self.tracks.len() {
            self.current = Some(index);
        }
        self.current_track()
    }
    #[cfg(test)]
    pub fn next(&mut self) -> Option<&Track> {
        let next = self
            .current
            .map(|i| (i + 1) % self.tracks.len())
            .unwrap_or(0);
        self.select(next)
    }
    #[cfg(test)]
    pub fn previous(&mut self) -> Option<&Track> {
        let previous = self
            .current
            .map(|i| if i == 0 { self.tracks.len() - 1 } else { i - 1 })
            .unwrap_or(0);
        self.select(previous)
    }
    pub fn current_track(&self) -> Option<&Track> {
        self.current.and_then(|i| self.tracks.get(i))
    }
    pub fn current_index(&self) -> Option<usize> {
        self.current
    }
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }
}

pub fn is_probably_media(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|x| x.to_str())
            .map(|x| x.to_ascii_lowercase())
            .as_deref(),
        Some(
            "mp3"
                | "wav"
                | "flac"
                | "ogg"
                | "m4a"
                | "aac"
                | "mp4"
                | "mkv"
                | "webm"
                | "mov"
                | "avi"
                | "m4v"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn playlist_wraps_navigation() {
        let mut playlist = Playlist::default();
        playlist.add("one.mp3");
        playlist.add("two.mp3");
        assert_eq!(playlist.next().unwrap().title, "two.mp3");
        assert_eq!(playlist.next().unwrap().title, "one.mp3");
        assert_eq!(playlist.previous().unwrap().title, "two.mp3");
    }
    #[test]
    fn recognizes_common_media() {
        assert!(is_probably_media(Path::new("clip.MKV")));
        assert!(!is_probably_media(Path::new("notes.txt")));
    }
}
