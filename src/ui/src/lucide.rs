//! Lucide icon definitions embedded for the native Rune UI.
//!
//! The geometry is sourced from lucide v1.8.0 (ISC), available at
//! <https://lucide.dev/icons>. Keeping the small set used by Veyra here avoids
//! a runtime asset dependency while preserving one consistent icon system.

use rune::Icon;

macro_rules! lucide {
    ($name:literal, $body:literal) => {
        Icon {
            name: $name,
            view: 24.0,
            body: concat!(
                r#"<g fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2">"#,
                $body,
                "</g>"
            ),
        }
    };
}

pub static PLAY: Icon = lucide!(
    "lucide-play",
    r#"<path d="M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z"/>"#
);
pub static PAUSE: Icon = lucide!(
    "lucide-pause",
    r#"<rect width="5" height="18" x="14" y="3" rx="1"/><rect width="5" height="18" x="5" y="3" rx="1"/>"#
);
pub static SKIP_BACK: Icon = lucide!(
    "lucide-skip-back",
    r#"<path d="M17.971 4.285A2 2 0 0 1 21 6v12a2 2 0 0 1-3.029 1.715l-9.997-5.998a2 2 0 0 1-.003-3.432z"/><path d="M3 20V4"/>"#
);
pub static SKIP_FORWARD: Icon = lucide!(
    "lucide-skip-forward",
    r#"<path d="M21 4v16"/><path d="M6.029 4.285A2 2 0 0 0 3 6v12a2 2 0 0 0 3.029 1.715l9.997-5.998a2 2 0 0 0 .003-3.432z"/>"#
);
pub static REWIND: Icon = lucide!(
    "lucide-rewind",
    r#"<path d="M12 6a2 2 0 0 0-3.414-1.414l-6 6a2 2 0 0 0 0 2.828l6 6A2 2 0 0 0 12 18z"/><path d="M22 6a2 2 0 0 0-3.414-1.414l-6 6a2 2 0 0 0 0 2.828l6 6A2 2 0 0 0 22 18z"/>"#
);
pub static FAST_FORWARD: Icon = lucide!(
    "lucide-fast-forward",
    r#"<path d="M12 6a2 2 0 0 1 3.414-1.414l6 6a2 2 0 0 1 0 2.828l-6 6A2 2 0 0 1 12 18z"/><path d="M2 6a2 2 0 0 1 3.414-1.414l6 6a2 2 0 0 1 0 2.828l-6 6A2 2 0 0 1 2 18z"/>"#
);
pub static VOLUME_2: Icon = lucide!(
    "lucide-volume-2",
    r#"<path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/><path d="M16 9a5 5 0 0 1 0 6"/><path d="M19.364 18.364a9 9 0 0 0 0-12.728"/>"#
);
pub static VOLUME_X: Icon = lucide!(
    "lucide-volume-x",
    r#"<path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/><path d="m22 9-6 6"/><path d="m16 9 6 6"/>"#
);
pub static FULLSCREEN: Icon = lucide!(
    "lucide-fullscreen",
    r#"<path d="M3 7V5a2 2 0 0 1 2-2h2"/><path d="M17 3h2a2 2 0 0 1 2 2v2"/><path d="M21 17v2a2 2 0 0 1-2 2h-2"/><path d="M7 21H5a2 2 0 0 1-2-2v-2"/><rect width="10" height="8" x="7" y="8" rx="1"/>"#
);
pub static INFO: Icon = lucide!(
    "lucide-info",
    r#"<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>"#
);
pub static TEXT_ALIGN_JUSTIFY: Icon = lucide!(
    "hugeicons-text-align-justify",
    r#"<path d="M4 6h16M4 10h16M4 14h16M4 18h16"/>"#
);
pub static FOLDER_OPEN: Icon = lucide!(
    "lucide-folder-open",
    r#"<path d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"/>"#
);
pub static X: Icon = lucide!(
    "lucide-x",
    r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#
);
