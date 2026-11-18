# Lua API 1.0

The global `veyra` table exposes `version()`, `log(message)`, `get_playback_info()`, `get_statistics()`, and `on(event_name, callback)`. Supported events are `playback_started`, `playback_paused`, `playback_resumed`, `playback_stopped`, `playback_finished`, `playback_seeked`, `playback_speed_changed`, and `track_changed`. Unknown event names are rejected. Callback errors are logged and do not stop playback.

Unavailable values are `nil`, not zero. The API exposes no filesystem, process, or shell functions.
