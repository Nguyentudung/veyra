# Roadmap and audit

This document incorporates the initial implementation audit. It distinguishes source-backed facts from unverified behavior.

## Verified implementation

- Modular Rust/GPUI application startup.
- Independent FFmpeg demux/codec path with no complete media-player subprocess.
- Software video conversion to BGRA and GPUI image handoff.
- libswresample plus ALSA audio path with volume/mute scaling.
- Worker shutdown and native resource cleanup boundaries.
- Playlist model and two passing unit tests.

## Partial or unverified

- Runtime audio/video playback has not been verified against committed media fixtures.
- Position and duration fields exist but are not populated.
- Relative seek semantics are not correct for all positions/containers until a current clock and stream time bases are used.
- There is no bounded buffering, audio master clock, late-frame dropping, or underrun recovery.
- Linux is intended but not covered by automated GUI/device verification.

## Priorities

1. Add deterministic audio/video fixtures and engine integration tests.
2. Track stream time bases, populate duration/position, and correct relative seeking.
3. Add bounded packet/audio/video queues and an audio-master/video-deadline scheduler.
4. Verify ALSA and GPUI behavior on Ubuntu with representative formats.
5. Add timeline, keyboard shortcuts, drag/drop, next/previous, and playback completion navigation.
6. Add subtitles, track selection, metadata, platform backends, and rendering/frame reuse.
