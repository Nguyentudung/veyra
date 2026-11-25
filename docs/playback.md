# Playback pipeline

## Status summary

- Container opening and stream discovery: **Implemented**.
- Audio/video packet decoding: **Implemented**.
- GPUI BGRA image handoff: **Implemented**.
- ALSA output and basic volume/mute scaling: **Partially Implemented**.
- Pause and command plumbing: **Partially Implemented**.
- Timestamp-based video pacing, duration reporting, and seek clock reset: **Implemented; real-media verification still needed**.
- Shared A/V clock, bounded packet/audio queues, late-frame policy, and underrun recovery: **Planned**.

## Opening and demuxing

`src/ui/src/lib.rs` invokes the Linux `zenity` chooser, applies the extension filter in `playlist::is_probably_media`, adds the track, and sends `Engine::open`. `src/video/src/decoder/demuxer.rs` calls `avformat_open_input`, `avformat_find_stream_info`, allocates one `AVPacket`, and reads packets with `av_read_frame`.

`src/video/src/decoder/codec.rs` calls `av_find_best_stream` for audio and video, copies codec parameters into contexts, and opens decoders. `Decoder::step` routes packets by stream index and unreferences them after processing.

## Audio path

`src/video/src/audio/decoder.rs` uses `avcodec_send_packet`/`avcodec_receive_frame`, then creates an `AudioOutput` on the first frame. `audio/resampler.rs` configures libswresample for stereo, 48 kHz, S16 output. `audio/decoder.rs` applies snapshot volume/mute scaling and writes interleaved samples through ALSA. `audio/output.rs` owns device and resampler cleanup. ALSA is configured for the actual resampler output: 48 kHz, two channels, S16 little endian. The earlier source-channel configuration could mismatch the stereo buffer for mono or multichannel media. PCM write errors are now surfaced, and recoverable ALSA errors are retried through `snd_pcm_recover` with bounded diagnostics.

## Video path

`src/video/src/rendering/decoder.rs` drains decoded frames. The GPUI frontend uses libswscale BGRA conversion and receives the latest frame through a bounded one-frame mailbox; timestamp pacing occurs before publication. GPUI then turns the bytes into a `RenderImage` for the video canvas.

## Timing

Video frames now use `best_effort_timestamp` (falling back to `pts`) multiplied by the decoded stream time base. `PresentationClock` anchors the first timestamp to `Instant` and waits until each subsequent frame's presentation deadline before publishing it to the latest-frame mailbox. The mailbox coalesces UI wakeups, so presentation follows source frame arrival instead of a fixed UI polling cadence; a 60 FPS source is no longer capped by a 33 ms timer. Paused, idle, and audio-only state uses a 250 ms status refresh. Opening a file creates a fresh clock; seeking resets the clock after the demuxer and codec state are flushed.

The current implementation keeps Veyra's existing lightweight architecture and does not copy mpv code or add libmpv. Audio output remains ALSA-paced, while video-only playback uses the timestamp clock. The latest-wins frame mailbox bounds memory, and GPUI does not decode or rebuild the full playback chrome at the video's cadence. Statistics materialization is similarly limited to 250 ms intervals while render-frame accounting remains per frame. A future profiling pass can add late-frame dropping and a shared audio master clock if real media testing shows that they are needed. No C implementation was reused beyond the existing FFmpeg, swscale, swresample, and ALSA libraries already integrated by Veyra.

## Controls and lifecycle

`src/video/src/commands.rs` carries open, pause, seek, volume, mute, and stop commands. `src/video/src/worker.rs` replaces the decoder on open, toggles the worker loop on pause, flushes codecs on seek, and joins on engine drop. End of input currently becomes an error-like completion state rather than automatically advancing the playlist.

Relative seek resolves the requested delta against the current snapshot position. Timeline interaction uses a separate absolute `SeekTo` command, so repeated drag events do not accumulate deltas against a stale position. Both paths call `avformat_seek_file` in AV_TIME_BASE units, call `avformat_flush`, flush audio/video codec buffers, discard the pending video frame, drop queued ALSA samples, and reset the presentation clock. `Snapshot.position` and `duration` are populated from the worker loop.

## Buffering and synchronization

The implementation has an mpsc command queue and a latest-frame mutex slot, but no bounded packet queue, decoded-audio queue, audio clock, video clock, late-frame drop policy, or underrun recovery. The one-frame slot is intentionally latest-wins, so it can discard frames when conversion or GPUI image upload falls behind. The current synchronous packet loop, software swscale conversion and per-frame BGRA allocation remain the likely stutter sources; this pass did not claim to make playback mpv-equivalent without real-media measurements.

## Compatibility and testing

The extension filter recognizes common MP3, WAV, FLAC, OGG, M4A, AAC, MP4, MKV, WebM, MOV, AVI, and M4V names. Actual support depends on installed FFmpeg decoders. Automated verification covers formatting, offline compilation, and workspace unit tests. Real-media playback, seek while playing/paused, FPS, CPU/memory and audio-sync comparisons remain unverified because this environment cannot operate the native file chooser or collect a comparable mpv run.
