# Veyra vs mpv benchmark

`benchmark.py` automates a like-for-like desktop playback comparison between
Veyra and mpv. It measures:

- startup: process launch until the first video output is ready;
- CPU: total process CPU time over the sampling window, from `/proc/<pid>/stat`;
- RAM: RSS, PSS, anonymous PSS, file PSS, and private dirty memory from
  `/proc/<pid>/smaps_rollup`.
- playback telemetry and FPS: Veyra delivery FPS from output-frame deltas;
  mpv's refresh-capped display-sync estimate when it is available; and
  player-specific frame-drop counters.

It needs Linux, Python 3.10 or newer, `mpv`, `matplotlib`, and an already-built
Veyra release binary. Install the chart dependency once:

```sh
# Ubuntu/Debian
sudo apt install python3-matplotlib

# or with pip
python3 -m pip install -r benchmarks/requirements.txt
```

```sh
# Build and run the comparison through the reusable target.
make benchmark

# Or invoke the harness directly.
cargo build --release
python3 -B benchmarks/benchmark.py --runs 3

# Run deterministic checks only.
make test
```

The default fixture is `video_test/test_02.mp4`; it is 1080×1920 at 60 FPS.
The default five-second sample window begins after a two-second warmup, which
keeps each run within that fixture's duration. Every generated artifact stays
outside the build directory in the repository-root `log/` folder:

- `log/veyra-vs-mpv.json` — structured benchmark data log;
- `log/veyra-vs-mpv.png` — bar-chart summary generated from that JSON log;
- `log/veyra-vs-mpv-timeline.png` — line charts showing sampled CPU, RSS, PSS,
  and available playback FPS over time;
- `log/logs/` — the captured stdout/stderr for each player run.

The white-background bar-chart summary has one panel per criterion. Each panel
compares the median across completed runs for Veyra and mpv, with the criterion,
statistic, unit, and preferred direction stated directly on the chart.

- **A. Startup latency** — process launch to first video output; unit: ms.
- **B. CPU utilisation** — total CPU time of all player threads during the
  sampling window; unit: % of one CPU core.
- **C. RSS memory** — per-run median resident set size during playback; unit:
  MiB.
- **D. PSS memory** — per-run median proportional set size during playback;
  unit: MiB. PSS apportions shared pages and is the more useful comparison
  measure.
- **E. Playback FPS** — Veyra's delivered-video rate and, when display sync is
  active, mpv's refresh-capped display-sync estimate; unit: frames/s. Higher is better,
  but see the measurement definitions below before treating the values as a
  like-for-like compositor-present count.

The separate timeline PNG has line charts for interval CPU utilisation, RSS,
PSS, and playback FPS. Each line is the median at a sampling position across
completed runs; its shaded band spans the minimum to maximum. Startup is a
one-time measurement, so it is included only in the bar-chart summary. The
benchmark records interval CPU, memory, and playback samples at
`--sample-interval` (250 ms by default).

### FPS and drop-counter semantics

The FPS panel compares useful player telemetry, not a confirmed physical-display
scan-out counter:

- **Veyra delivery FPS** is the delta of its `rendered_frames` output counter
  divided by elapsed sampling time. It means video frames delivered to Veyra's
  `NativeSurface` per second. It is not proof that a compositor or monitor
  presented every one of those frames.
- **mpv display-bound FPS** is an estimate calculated from active display-sync
  telemetry as `min(estimated-display-fps, estimated-display-fps / vsync-ratio)`.
  The cap avoids claiming more physical presentations than the measured display
  refresh; the uncapped scheduler cadence is retained as
  `display_sync_scheduled_fps`. It is reported only while mpv says display sync
  is active. With Xvfb, a headless/no-vsync backend, or another output without
  active display sync, mpv FPS is deliberately **N/A**; the FPS bar and
  timeline line can therefore be absent.
- Veyra also records the sampled-window `frame_drop_count` (and rate), but it
  is the estimate `processed_frames - delivered_frames`. It includes deliberate
  latest-wins mailbox coalescing when source FPS exceeds the UI/display rate,
  so it is not a compositor drop counter. mpv records separate output
  (`vo_frame_drop_count`) and decoder (`decoder_frame_drop_count`) deltas.
  Keep those counters separate: they are diagnostic signals from different
  player pipelines, not one interchangeable total.

The default `--mpv-video-sync=display-resample` is passed through to mpv as
`--video-sync=display-resample`. Change it when experimenting with an mpv video
sync policy, and record the choice in the JSON configuration:

```sh
python3 -B benchmarks/benchmark.py --runs 5 \
  --mpv-video-sync=display-resample
```

For a display-bound FPS comparison, run both players on the same normal
vsync-capable graphical display. A headless Xvfb run remains useful for startup,
CPU, memory, Veyra delivery FPS, and drop counters, but it is not an mpv
display-bound-FPS comparison.

To render a chart again from a saved log without re-running playback:

```sh
python3 benchmarks/render_chart.py \
  --input log/veyra-vs-mpv.json \
  --output log/veyra-vs-mpv.png \
  --timeline-output log/veyra-vs-mpv-timeline.png
```

By default mpv receives `--no-config --hwdec=no`, matching Veyra's current
software-decoding path and avoiding personal mpv configuration. To compare a
hardware-decoded mpv setup explicitly, choose its policy and record it in the
result:

```sh
python3 -B benchmarks/benchmark.py --mpv-hwdec=auto-safe
```

To compare decoder-threading policies, pass both values explicitly; the chosen
policy is stored in the result configuration:

```sh
python3 -B benchmarks/benchmark.py --runs 5 \
  --veyra-threads 4 --veyra-thread-mode frame
```

For a different fixture, ensure that `warmup + sample` is shorter than the
media duration:

```sh
python3 -B benchmarks/benchmark.py \
  --media /absolute/path/to/video.mp4 \
  --warmup-seconds 3 --sample-seconds 10 --runs 5
```

Run it on an otherwise idle desktop, use the same display/backend for both
players, and compare summary medians as well as the time-series shape rather
than a single run. A graphical session is required for an mpv display-bound-FPS
comparison. In headless automation, provide Xvfb/Mesa yourself, for example
`make benchmark-headless`; expect mpv display-bound FPS to be `N/A` there. That
target clears inherited Wayland socket variables and forces Rune/winit to X11;
wrapping `make benchmark` in `xvfb-run` alone can still select a parent Wayland
session instead of Xvfb.
