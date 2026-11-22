use super::{
    events::PlaybackEvent,
    runtime::{RuntimeHandle, script_directory},
};
use std::fs;
use veyra_statistics::StatisticsSnapshot;

pub struct ScriptManager {
    runtime: RuntimeHandle,
}
impl ScriptManager {
    pub fn new(stats: StatisticsSnapshot) -> Self {
        let directory = script_directory();
        let _ = fs::create_dir_all(directory.join("tools"));
        let _ = fs::create_dir_all(directory.join("player"));
        Self {
            runtime: RuntimeHandle::start(directory, stats),
        }
    }
    pub fn reload(&self) {
        self.runtime.reload();
    }
    pub fn dispatch(&self, event: PlaybackEvent, stats: StatisticsSnapshot) {
        self.runtime.event(event.payload(&stats), stats);
    }
    /// Evaluates a single Lua chunk on the script thread. Results and errors
    /// are intentionally printed to stderr for developer debugging.
    pub fn eval(&self, source: String) {
        self.runtime.eval(source)
    }
}
