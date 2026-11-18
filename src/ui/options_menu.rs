//! Bottom-left tool menu and compact native-style popup.

use super::{State, component_impl};
use rune::El;
use rune_kit::Kit;

pub(crate) fn trigger(state: &State, kit: &Kit) -> El {
    component_impl::options_button(state, kit)
}
pub(crate) fn view(state: &State, kit: &Kit) -> El {
    component_impl::options_view(state, kit)
}
