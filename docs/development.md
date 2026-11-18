# Development and verification

## Build

On Ubuntu install:

```sh
sudo apt install ffmpeg libavformat-dev libavcodec-dev libavutil-dev libswscale-dev libswresample-dev libasound2-dev zenity clang libclang-dev
cargo run
cargo test --offline
```

`build.rs` reads `wrapper.h`, generates bindings with bindgen, and links FFmpeg/ALSA libraries. Rune and Rune Kit are pinned git dependencies; the patch entries mirror Rune's own workspace patches so the app uses the same native integration crates.

## Conventions

Keep UI state and intent in `src/ui/`, playlist state in `src/playlist/`, the playback façade in `src/playback/`, and native media ownership in `src/video/`. Do not pass native decoder/device pointers or worker-owned Rune objects across the boundary. Keep blocking decode/audio work on the named worker. When a feature or dependency changes, update the relevant section here or in the focused document and add verification evidence.

## Testing

The Rune runtime provides deterministic headless rendering and tree inspection, but Veyra's automated tests remain playlist-only. Headless rendering validates the empty-state layout and accessibility tree, including the corner controls and central transport labels. The suite does not yet open real media, inspect decoded frames, exercise ALSA, validate end-to-end seeking, measure audio/video synchronization, or run a desktop GUI; those checks require an interactive media-open session.

Add deterministic short audio/video fixtures and test decoder startup, frame production, audio conversion, repeated open/close, unsupported files, and seek behavior without requiring a desktop window in CI.

## Performance and maintainability

Software decode/conversion and per-presented-frame BGRA allocation are current hot paths. The latest-frame slot is bounded to one frame but not by frame size, and blocking ALSA writes can dominate worker time. Rune consumes the moved frame through its own latest-wins native surface; profile before introducing frame reuse, bounded queues, or hardware acceleration.
