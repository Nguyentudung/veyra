//! Small monochrome icons embedded in the executable.

use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let body = match path {
            "media" => {
                r#"<rect x="3" y="4" width="18" height="16" rx="3"/><path d="m10 9 5 3-5 3z"/>"#
            }
            "folder" => {
                r#"<path d="M3 6a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M3 10h18"/>"#
            }
            "music" => {
                r#"<path d="M9 18V6l11-2v12M9 9l11-2"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="17.5" cy="16" r="2.5"/>"#
            }
            "previous" => r#"<path d="M6 5v14M19 6l-9 6 9 6z"/>"#,
            "next" => r#"<path d="M18 5v14M5 6l9 6-9 6z"/>"#,
            "rewind" => r#"<path d="M11 6 3 12l8 6zM21 6l-8 6 8 6z"/>"#,
            "forward" => r#"<path d="m3 6 8 6-8 6zM13 6l8 6-8 6z"/>"#,
            "play" => r#"<path d="m9 5 10 7-10 7z"/>"#,
            "pause" => r#"<path d="M9 5v14M15 5v14"/>"#,
            "volume" => {
                r#"<path d="M4 9v6h4l5 4V5L8 9zM17 9a5 5 0 0 1 0 6M19.5 6.5a8 8 0 0 1 0 11"/>"#
            }
            "muted" => r#"<path d="M4 9v6h4l5 4V5L8 9zM17 9l4 6M21 9l-4 6"/>"#,
            "fullscreen" => r#"<path d="M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5"/>"#,
            "info" => r#"<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 8h.01"/>"#,
            "menu" => r#"<path d="M4 7h16M4 12h16M4 17h16"/>"#,
            "playlist" => r#"<path d="M4 6h12M4 11h12M4 16h8M18 14l3 2-3 2z"/>"#,
            "close" => r#"<path d="m6 6 12 12M18 6 6 18"/>"#,
            "minimize" => r#"<path d="M5 12h14"/>"#,
            "maximize" => r#"<rect x="5" y="5" width="14" height="14" rx="1"/>"#,
            _ => return Ok(None),
        };
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
        );
        Ok(Some(Cow::Owned(svg.into_bytes())))
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}
