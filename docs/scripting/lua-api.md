# Lua API 1.0

The global `veyra` table exposes `version()`, `log(message)`, `benchmark()`, `get_playback_info()`, `get_statistics()`, and `on(event_name, callback)`. `log` and `benchmark` write to stderr. `benchmark()` prints the current measurements shown in Info → Performance; it does not execute a separate benchmark or claim GPU execution time. Supported events are `playback_started`, `playback_paused`, `playback_resumed`, `playback_stopped`, `playback_finished`, `playback_seeked`, `playback_speed_changed`, and `track_changed`. Unknown event names are rejected. Callback errors are logged and do not stop playback.

Open **Lua console** from the bottom-left options menu. It is a single-line REPL prompt like mpv's console: enter a Lua expression or statement and press Enter/Run. Results, `veyra.log` output, and errors are printed to the terminal that launched Veyra. Press `R` or select **Reload tools** after editing a tool.

Unavailable values are `nil`, not zero. The API exposes no filesystem, process, or shell functions.
