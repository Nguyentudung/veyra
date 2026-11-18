//! Shared glass and control dimensions used by the video overlays.

use rune::{El, color::hex, row};

pub const GLASS_BLUR: f32 = 12.0;
pub const SURFACE_OPAQUE: &str = "#262629";
pub const SURFACE_BORDER: &str = "#ffffff1a";
pub const GLASS_SURFACE: &str = "#121216a6";

/// Wraps an icon control in the app's translucent, fully rounded glass surface.
pub fn glass_capsule(child: El) -> El {
    row()
        .px(8.0)
        .py(8.0)
        .radius(999.0)
        .backdrop_blur(GLASS_BLUR)
        .fill(hex("#121216a6").unwrap())
        .border(1.0, hex("#ffffff1f").unwrap())
        .hover(hex("#ffffff24").unwrap())
        .child(child)
}
