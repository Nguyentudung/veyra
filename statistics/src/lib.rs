//! Playback measurements, aggregation, and bounded realtime history.

mod aggregator;
mod collector;
mod history;
mod model;

pub use collector::StatisticsCollector;
pub use model::StatisticsSnapshot;
