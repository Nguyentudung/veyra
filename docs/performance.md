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
  than allocating and uploading a CPU BGRA frame for normal playback. NV12
  keeps its two-plane path; decoded YUV420P now uploads Y, Cb and Cr directly
  to three GPU planes, avoiding the previous per-frame CPU Cb/Cr interleave
  and its reusable 4K-scale scratch buffer.
- Rune's GPU context is handed to the media worker on every UI wake, including
  frame-only wakes. This lets a stream switch from its startup fallback to the
  direct Y/UV path as soon as Rune finishes its first draw, rather than waiting
  for the 250 ms controls refresh.
- The software fallback no longer zero-fills an output frame just before
  libswscale overwrites it. It exposes that allocation only after FFmpeg has
  produced all visible rows.
- The direct YUV path starts with three leased texture slots and allocates a
  fourth only under contention. This saves one full YUV420 picture (about 3 MiB
  at 1080×1920) when three suffice, while bounding memory and dropping a
  frame if a stalled compositor exhausts all four slots.
- Lua script directory scanning and runtime creation are deferred until a
  stream has its first frame (or one second for audio-only media). Explicit
  reload/console use starts it immediately.
- Source-paced video sleeps for most of a frame interval, then spends up to
  250 microseconds spinning for its deadline. This intentionally consumes a
  small amount of extra CPU to reduce scheduler jitter without busy-waiting for
  an entire frame.
- Benchmark telemetry is opt-in and sampled at the existing 250 ms statistics
  cadence, so ordinary launches do not incur file-writing work.

## FPS telemetry semantics

The benchmark keeps media progress separate from delivery and display timing.
None of these values alone is a universal physical-display presentation counter.

- **CFR media-clock FPS estimate** (`media_clock_fps_estimate`) is emitted for
  both Veyra and mpv as nominal ffprobe source FPS × `Δmedia_time / Δwall_time`.
  It is available in headless runs and is the common FPS panel/chart metric.
  It is intentionally absent for VFR or unknown-rate media rather than being
  fabricated from an unreliable nominal FPS.
- **Veyra delivery FPS** (`delivery_fps`) is the delta of its
  `rendered_frames` output counter across the measurement window. It represents
  frames handed to `NativeSurface` per second, not confirmed compositor scanout.
- **mpv display-sync telemetry** has no presentation-FPS label. When active,
  `display_sync_scheduled_cadence_fps = estimated_display_fps / vsync_ratio`
  is a scheduler/source cadence that may exceed physical refresh because drops
  can contribute zero vsync. Under Xvfb/no vblank it is absent with
  `no_display_sync`. The benchmark instead still reports its CFR media-clock
  rate and the explicitly modelled `source_frame_admission_fps_estimate`.
- Drop counters remain available regardless of display sync. Veyra's
  `frame_drop_count` is `processed_frames - delivered_frames`, so it includes
  intentional latest-wins mailbox coalescing. mpv records separate output
  (`vo_frame_drop_count`) and decoder (`decoder_frame_drop_count`) deltas.
  Treat them as diagnostics, not one interchangeable total.

## RAM telemetry and headless runs

PSS is the primary cross-process footprint metric; RSS is supplementary. The
benchmark also records `Pss_Shmem` so GPU-driver/memfd texture allocations are
visible separately from anonymous application heap.

`make benchmark-headless` is marked `headless_diagnostic`. Xvfb/Mesa can run
Veyra through Vulkan/lavapipe shared memory while mpv falls back to an X11 VO;
that makes absolute RAM samples useful for diagnosis but not an honest Veyra vs
mpv RAM ranking. Use `make benchmark` in a normal desktop session with matched
backend/VO/AO policy for an end-to-end footprint comparison.

`--mpv-video-sync` selects the mpv `--video-sync` policy; its default is
`display-resample`, which asks mpv for display-synchronised telemetry when the
video output supports it. Keep the same policy for every run and retain it in
the JSON configuration:

```sh
python3 -B benchmarks/benchmark.py --runs 5 \
  --mpv-video-sync=display-resample
```

Use a normal, vsync-capable graphical session when comparing display timing.
Xvfb/Mesa remains suitable for controlled smoke diagnostics and common CFR
media-clock FPS, but not for cross-player renderer RAM or physical-display FPS.

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

`benchmark-headless` clears inherited Wayland socket variables, forces
Rune/winit to X11, and records `--headless` before starting Xvfb. Do not simply
wrap `make benchmark` in `xvfb-run` from a Wayland shell: winit can otherwise
select the parent Wayland session instead of the Xvfb display.

The JSON artifact records startup, process CPU, RSS/PSS/PSS-shared, Veyra
delivery telemetry (including rendered/processed frames, output-delta FPS,
interval and jitter, conversion timings, drop counters, and effective decoder
threads), and mpv media-clock, drop, VO/AO, and display-sync diagnostics. Under
Xvfb/no vsync the display-sync cadence is absent, while CFR media-clock FPS is
still reported for both players.
Performance runs are intentionally informational rather than a hard CI gate:
GPU drivers, CPU frequency policy, and desktop compositors make small
cross-machine deltas unreliable. Compare several runs on the same idle host
and keep the JSON artifact with the change being evaluated.
