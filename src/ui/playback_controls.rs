//! Bottom timeline, transport controls, and volume control.

use super::{State, controls, timeline, volume_control};
use rune::El;
use rune_kit::Kit;

pub(crate) fn transport(state: &State, kit: &Kit) -> El {
    controls(state, kit)
}
pub(crate) fn scrubber(state: &State, kit: &Kit) -> El {
    timeline(state, kit)
}
pub(crate) fn volume(state: &State, kit: &Kit) -> El {
    volume_control(state, kit)
}
