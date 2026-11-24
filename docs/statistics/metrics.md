# Metrics

Implemented metrics are file name/path, container extension, file size, position,
duration, remaining time, playback state, playback speed (the engine's fixed
normal speed), video codec/profile/pixel format, resolution, stream FPS, codec
bitrate where FFmpeg exposes it, decoder implementation, effective FFmpeg video
decode-thread count/mode, audio codec/bitrate/sample rate/channels, ALSA output,
video frames processed/rendered, render FPS, frame interval/jitter, estimated
dropped frames, current/average/peak software conversion time, and audio
processing time. Frame processing history is bounded to 120 samples and
exposed to Lua. The benchmark sidecar records the playback performance metrics
at the same 250 ms cadence when explicitly enabled.

Packet queues, buffering, GPU timing, color metadata, and all-stream
enumeration remain unavailable because the current engine does not measure or
expose them.
