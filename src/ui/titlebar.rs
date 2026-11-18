//! Window chrome integration.
//!
//! Veyra intentionally keeps Rune's native titlebar and window controls. This
//! module is the seam for future custom chrome without duplicating platform
//! minimize/maximize/close behavior in the player canvas.

use super::State;
use rune::{Cx, El, el};
use rune_kit::Kit;

pub(crate) fn overlay(_state: &State, _kit: &Kit, _cx: &mut Cx<State>) -> El {
    el()
}
