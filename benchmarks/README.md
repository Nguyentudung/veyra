# Veyra vs mpv benchmark

`benchmark.py` automates a like-for-like desktop playback comparison between
Veyra and mpv. It measures:

- startup: process launch until the first video output is ready;
- CPU: total process CPU time over the sampling window, from `/proc/<pid>/stat`;
- RAM: RSS, PSS, anonymous/file/shared PSS, and private dirty memory from
  `/proc/<pid>/smaps_rollup`.
- playback telemetry and FPS: a shared CFR media-clock FPS estimate for both
  players (including Xvfb), Veyra delivery FPS, mpv admission/drop diagnostics,
  and optional display-sync scheduling diagnostics.

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
  shared PSS, and CFR media-clock FPS over time;
- `log/logs/` — the captured stdout/stderr for each player run.

The white-background bar-chart summary has one panel per criterion. Each panel
compares the median across completed runs for Veyra and mpv, with the criterion,
statistic, unit, and preferred direction stated directly on the chart.

- **A. Startup latency** — process launch to first video output; unit: ms.
- **B. CPU utilisation** — total CPU time of all player threads during the
  sampling window; unit: % of one CPU core.
- **C. PSS memory (primary)** — per-run median proportional set size during
  playback; unit: MiB. PSS apportions shared pages and is the primary footprint
  comparison measure.
- **D. Shared PSS / GPU memory** — per-run `Pss_Shmem`; unit: MiB. This exposes
  memfd, driver, and texture allocations that are not Rust heap allocations.
- **E. RSS memory (supplementary)** — per-run median resident set size; unit:
  MiB. It includes shared pages in full and should not decide a close comparison.
- **F. CFR media-clock FPS** — nominal fixture FPS multiplied by media-time
  progress over wall time; unit: frames/s. It works without display sync, but is
  not a compositor-present count.

The separate timeline PNG has line charts for interval CPU utilisation, RSS,
PSS, shared PSS, and CFR media-clock FPS. Each line is the median at a sampling position across
completed runs; its shaded band spans the minimum to maximum. Startup is a
one-time measurement, so it is included only in the bar-chart summary. The
benchmark records interval CPU, memory, and playback samples at
`--sample-interval` (250 ms by default).

### FPS and drop-counter semantics

The FPS panel compares controlled-fixture progress, not a confirmed
physical-display scan-out counter:

- **CFR media-clock FPS estimate** is calculated for both players as nominal
  ffprobe source FPS × `Δmedia_time / Δwall_time`. It is emitted only when
  `avg_frame_rate == r_frame_rate`; VFR/unknown fixtures report it as **N/A**.
  It remains available under Xvfb, which fixes the old mpv `N/A` gap without
  claiming that a frame was physically presented.
- **Veyra delivery FPS** is the delta of its `rendered_frames` output counter
  divided by elapsed sampling time. It means video frames delivered to Veyra's
  `NativeSurface` per second. It is not proof that a compositor or monitor
  presented every one of those frames.
- **mpv display-sync telemetry** is deliberately diagnostic only. When active,
  `display_sync_scheduled_cadence_fps = estimated_display_fps / vsync_ratio` is
  retained as a scheduler/source cadence. It can exceed physical refresh and is
  never labelled presentation FPS. Under Xvfb/no vblank it is absent with the
  explicit `no_display_sync` reason; the shared CFR media-clock estimate still
  remains available.
- **mpv source-frame admission estimate** subtracts VO and decoder drop rates
  from the CFR media-clock rate. It is a modelled pipeline-health value, not a
  successful-display-flip counter.
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

For display timing diagnostics, run both players on the same normal
vsync-capable graphical display. A headless Xvfb run remains useful for a
repeatable smoke/instrumentation pass, but it is not a cross-player renderer-RAM
or physical-display-FPS ranking: Xvfb/Mesa can select radically different VO,
GPU-driver, and shared-memory paths.

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
than a single run. A graphical session is required for physical-display timing.
In headless automation, use `make benchmark-headless`; its `--headless` marker
prints an explicit warning and records that RAM/display ranking is diagnostic
only. That target clears inherited Wayland socket variables and forces
Rune/winit to X11; wrapping `make benchmark` in `xvfb-run` alone can still
select a parent Wayland session instead of Xvfb.
