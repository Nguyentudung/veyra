# Development and verification

## Build

On Ubuntu install:

```sh
sudo apt install ffmpeg libavformat-dev libavcodec-dev libavutil-dev libswscale-dev libswresample-dev libasound2-dev zenity clang libclang-dev
cargo run
cargo test --offline
cargo build --release
pidstat -u -r -p "$(pgrep -n veyra)" 1 30
```

`build.rs` reads `wrapper.h`, generates bindings with bindgen, and links FFmpeg/ALSA libraries. Rune and Rune Kit are vendored under `third_party/rune` (including Rune's original license); the patch entries point to Rune's vendored native integration crates. A normal clone therefore needs no sibling Rune checkout or Git submodule initialization.

## Conventions

Keep UI state and intent in `src/ui/src/`, playlist state in `src/playlist/src/`, the playback façade in `src/playback/src/`, and native media ownership in `src/video/src/`. Each first-party crate lives under `src/<crate>/`; benchmark/test-data tooling intentionally lives outside the application tree in `benchmarks/`. Do not pass native decoder/device pointers or worker-owned Rune objects across the boundary. Keep blocking decode/audio work on the named worker. When a feature or dependency changes, update the relevant section here or in the focused document and add verification evidence.

## Testing

Run all deterministic checks with `make test`, or directly with:

```sh
cargo test --workspace
python3 -B -m unittest discover -s benchmarks -p 'test_*.py' -v
```

The Rust suite currently covers playlist behavior plus cache, mailbox,
statistics, scripting, and decoder-threading policy. The Python suite verifies
the benchmark result/telemetry parser without requiring a desktop window.
Rune also provides deterministic headless rendering and tree inspection for UI
work.

Real media, ALSA, desktop composition, GPU upload, and audio/video
synchronization still require an interactive session or an Xvfb/Mesa runner.
Use `make benchmark` for that non-gating performance comparison; see
`docs/performance.md` for the environment requirements and interpretation.

## Performance and maintainability

The latest-frame slot is bounded to one frame but not by frame size, and
blocking ALSA writes can dominate worker time. Rune consumes the moved frame
through its own latest-wins native surface; profile before introducing frame
reuse, bounded queues, or hardware acceleration. Decoder threading and
benchmark methodology are documented in `docs/performance.md`.
