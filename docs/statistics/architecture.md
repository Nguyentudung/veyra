# Statistics architecture

`src/statistics` is independent of the Rune UI. `StatisticsCollector` samples the thread-safe playback `Snapshot` at 250 ms, stores bounded 120-sample position and frame-processing histories, and exposes a cloned `StatisticsSnapshot` to consumers. The collector never reads FFmpeg pointers and never blocks the decoder. Frame conversion timing and processed-frame counters are measured in the media worker around the existing software renderer.

Only values supplied by the current FFmpeg playback worker are exposed. Missing values remain `nil` in Lua and are shown as unavailable in the UI.
