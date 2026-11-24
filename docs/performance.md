# Performance, threading, and benchmarks

Veyra keeps the UI, media engine, and ALSA output in separate execution
domains. The UI thread never decodes media. The media engine owns the FFmpeg
format/codec contexts, while FFmpeg can create its own decoder workers for a
single video stream. This preserves packet and frame ordering without sharing
an `AVCodecContext` across Rust threads.

## Decoder CPU threading

Video decoding is multi-threaded by default. Veyra configures FFmpeg before
`avcodec_open2`; the actual active mode and effective number of threads appear
in the Statistics panel and Lua `veyra.get_statistics().video` table.

| Environment variable | Default | Meaning |
| --- | --- | --- |
| `VEYRA_VIDEO_THREADS` | `auto` | `auto` uses one fewer logical CPU, capped at 8; `1` selects a single decoder thread; a positive number is capped at available logical CPUs. |
| `VEYRA_VIDEO_THREAD_MODE` | `auto` | `auto` lets FFmpeg use every thread type supported by the codec; `frame` emphasizes throughput; `slice` can reduce decoding latency when the stream supports slice threading. |

Frame threading can buffer extra decoded frames, so use
`VEYRA_VIDEO_THREAD_MODE=slice` or `VEYRA_VIDEO_THREADS=1` when seek/start
latency is more important than peak decode throughput. A codec that does not
support the requested mode automatically falls back to its supported mode;
playback does not fail solely because of a threading preference.

## Runtime paths

- `gpu-video` remains the default renderer. It passes Y/UV data to Rune rather
  than allocating and uploading a CPU BGRA frame for normal playback.
- The software fallback no longer zero-fills an output frame just before
  libswscale overwrites it. It exposes that allocation only after FFmpeg has
  produced all visible rows.
- Lua script directory scanning and runtime creation are deferred until a
  stream has its first frame (or one second for audio-only media). Explicit
  reload/console use starts it immediately.
- Benchmark telemetry is opt-in and sampled at the existing 250 ms statistics
  cadence, so ordinary launches do not incur file-writing work.

## FPS telemetry semantics

The benchmark's two FPS values are intentionally source-specific; neither is a
universal physical-display presentation counter.

- **Veyra delivery FPS** (`playback_metrics.playback_fps`) is calculated from
  the delta of Veyra's `rendered_frames` output counter across the measurement
  window. It represents frames delivered to `NativeSurface` per second. It does
  not establish that the compositor or monitor scan-out presented every frame.
- **mpv display-bound FPS** is a display-sync estimate, calculated as
  `min(estimated-display-fps, estimated-display-fps / vsync-ratio)` only while
  mpv reports `display-sync-active`. The cap prevents a scheduler cadence above
  the measured physical refresh from being described as more presentations;
  the uncapped cadence remains available as `display_sync_scheduled_fps`. With
  Xvfb, no vsync, or an output backend without display sync, the benchmark
  writes no mpv FPS value: the terminal summary and chart show it as
  `N/A`/absent rather than inventing a comparable number.
- Drop counters remain available regardless of whether the mpv estimate is
  available. Veyra's `frame_drop_count` is `processed_frames - delivered_frames`,
  so it includes intentional latest-wins mailbox coalescing when source FPS
  exceeds UI/display cadence; it is not a confirmed compositor drop count. mpv
  records output (`vo_frame_drop_count`) and decoder
  (`decoder_frame_drop_count`) deltas. They describe different pipelines and
  should be compared as diagnostics, not summed into one cross-player count.

`--mpv-video-sync` selects the mpv `--video-sync` policy; its default is
`display-resample`, which asks mpv for display-synchronised telemetry when the
video output supports it. Keep the same policy for every run and retain it in
the JSON configuration:

```sh
python3 -B benchmarks/benchmark.py --runs 5 \
  --mpv-video-sync=display-resample
```

Use a normal, vsync-capable graphical session when comparing display-bound FPS.
Xvfb/Mesa is still suitable for repeatable startup, CPU, RAM, Veyra delivery
FPS, and drop-counter runs, but mpv display-bound FPS must be treated as `N/A`.

## Repeatable benchmark workflow

Run deterministic checks with:

```sh
make test
```

Run the desktop comparison (build time is excluded from the startup result):

```sh
make benchmark
# Equivalent with an explicit threading experiment:
python3 -B benchmarks/benchmark.py --build-veyra --runs 5 \
  --veyra-threads 4 --veyra-thread-mode frame
```

The comparison needs a graphical Linux session that both Veyra and mpv can
use. For headless automation, run it in a runner configured with Xvfb/Mesa,
for example:

```sh
make benchmark-headless
```

`benchmark-headless` clears inherited Wayland socket variables and forces
Rune/winit to X11 before starting Xvfb. Do not simply wrap `make benchmark` in
`xvfb-run` from a Wayland shell: winit can otherwise select the parent Wayland
session instead of the Xvfb display.

The JSON artifact records startup, process CPU, RSS/PSS, Veyra delivery
telemetry (including rendered/processed frames, output-delta FPS, interval and
jitter, conversion timings, drop counters, and effective decoder threads), and
mpv display-sync telemetry when it is available (display-bound FPS estimate and
its output/decoder drop counters). Under Xvfb or no vsync, the mpv display-bound
FPS is intentionally absent/`N/A`.
Performance runs are intentionally informational rather than a hard CI gate:
GPU drivers, CPU frequency policy, and desktop compositors make small
cross-machine deltas unreliable. Compare several runs on the same idle host
and keep the JSON artifact with the change being evaluated.
