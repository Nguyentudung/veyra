//! Embedded Lua scripting with a small, versioned and deliberately safe API.

mod api;
mod events;
mod manager;
mod runtime;

pub use events::PlaybackEvent;
pub use manager::ScriptManager;
