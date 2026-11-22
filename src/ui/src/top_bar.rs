//! Top-left fullscreen, centered filename pill, and top-right info control.

use super::{State, component_impl};
use rune::El;
use rune_kit::Kit;

pub(crate) fn view(state: &State, kit: &Kit) -> El {
    component_impl::top_bar(state, kit)
}
