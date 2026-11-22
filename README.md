# Veyra

Detailed technical documentation is organized in [docs/architecture.md](docs/architecture.md), [docs/playback.md](docs/playback.md), [docs/ui.md](docs/ui.md), [docs/platforms.md](docs/platforms.md), [docs/development.md](docs/development.md), and [docs/roadmap.md](docs/roadmap.md).

Veyra is a native desktop media player written in Rust with Rune and Rune Kit. First-party application code is consolidated under the root `src/` directory: each domain is an independent Cargo crate in `src/<crate>/`, while `src/main.rs` remains the executable entry point.

The main crates are:

- `src/app` — application bootstrap.
- `src/ui` — Rune UI and overlay composition.
- `src/playback` — engine façade and worker orchestration.
- `src/video` — FFmpeg/ALSA demuxing, decoding, rendering, and audio output.
- `src/statistics` and `src/scripting` — playback metrics and Lua tools.
- `src/playlist` and `src/utils` — reusable application helpers.
- `src/cache` — bounded memory reservations used by media queues.

## Features

- Local audio playback through FFmpeg/libswresample and ALSA
- Local video decoding through FFmpeg/libswscale and in-window Rune native-surface frames
- Pause/resume, relative seeking, previous/next track navigation, mute, volume, and native fullscreen
- Video-first desktop layout with floating transport controls and an opt-in playlist overlay
- Playlist selection and wraparound playlist model
- Error presentation and cleanup when switching media

Open media with the **Open media** button in the empty state. `zenity` is used as the Linux file chooser.

## Build on Ubuntu

Install development/runtime dependencies:

```sh
sudo apt install ffmpeg libavformat-dev libavcodec-dev libavutil-dev libswscale-dev libswresample-dev libasound2-dev zenity clang libclang-dev
cargo run
```

Run the workspace with `cargo run` and test all crates with `cargo test --workspace`.

## Benchmark Veyra against mpv

The test-data harness is documented in [benchmarks/README.md](benchmarks/README.md).
It is deliberately outside `src/`, because it is not part of the shipped
application. It uses Python plus `matplotlib` to write a JSON log and PNG chart
under the repository-root `log/` directory. After building Veyra, run:

```sh
python3 benchmarks/benchmark.py --runs 3
```

## Architecture and research

`src/playlist/src/lib.rs` contains application-owned playlist logic. `src/video/src/` owns the isolated media worker, FFmpeg format/codec contexts, ALSA output, resampling, GPU YUV upload with a software fallback, and the synchronized one-frame mailbox. `src/ui/src/lib.rs` is the Rune state/view boundary and never performs decode or audio work. Rune Kit supplies the buttons, icon buttons and semantic visual tokens; Rune's `NativeSurface` receives the latest frame without a second UI-side pixel buffer.

VLC, FFmpeg, Symphonia, and GStreamer source were evaluated for separation of input/demux, codec modules, clocks, outputs, and playlist orchestration. FFmpeg was selected for the Ubuntu MVP because its low-level libraries provide broad container/codec coverage. Veyra does not copy VLC, FFmpeg, Symphonia, or GStreamer source or assets.

## Dependency and license audit

- Veyra: Apache-2.0.
- Rune and Rune Kit: pinned to the `cca5df6fafb4721d96fe105f126ebfd2ec351bb6` development revision; both are pre-release MIT-licensed workspace crates.
- FFmpeg shared libraries: LGPL-2.1-or-later in the Ubuntu configuration; exact build flags must be audited before redistribution. Some optional configurations are GPL, and codecs may carry separate patent obligations.
- ALSA library: LGPL-2.1-or-later.
- `bindgen`: build-time dependency used for generated FFmpeg/ALSA FFI.
- Veyra links to system shared libraries and does not ship codec binaries.

## Memory management

`src/cache` provides an RAII `MemoryBudget`. The audio queue uses an 8 MiB budget in addition to its two-second frame limit, so queued samples cannot grow without bound. Reservations are released when queued chunks are dropped or played. This limits application-owned media buffers; the RSS reported by the OS also includes Rune, FFmpeg codec contexts, ALSA buffers, shared libraries, and allocator overhead, which a cache cannot reclaim while those components are active.

## Current limitations

Clock reporting and duration display, bounded packet/frame queues, drag-and-drop event wiring, subtitle/audio-track selection, and late-frame dropping remain next milestones. The current worker keeps decoding and blocking audio I/O off Rune's event loop and publishes frames through Rune's bounded latest-wins native surface.
