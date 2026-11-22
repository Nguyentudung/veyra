//! Statistics / media information popup and its tabs.

use super::{State, component_impl};
use rune::El;
use rune_kit::Kit;

pub(crate) fn view(state: &State, kit: &Kit) -> El {
    component_impl::statistics_view(state, kit)
}
