# Veyra vs mpv benchmark

`benchmark.py` automates a like-for-like desktop playback comparison between
Veyra and mpv. It measures:

- startup: process launch until the first video output is ready;
- CPU: total process CPU time over the sampling window, from `/proc/<pid>/stat`;
- RAM: RSS, PSS, anonymous PSS, file PSS, and private dirty memory from
  `/proc/<pid>/smaps_rollup`.

It needs Linux, Python 3.10 or newer, `mpv`, `matplotlib`, and an already-built
Veyra release binary. Install the chart dependency once:

```sh
# Ubuntu/Debian
sudo apt install python3-matplotlib

# or with pip
python3 -m pip install -r benchmarks/requirements.txt
```

```sh
cargo build --release
python3 benchmarks/benchmark.py --runs 3
```

The default fixture is `video_test/test_02.mp4`; it is 1080×1920 at 60 FPS.
The default five-second sample window begins after a two-second warmup, which
keeps each run within that fixture's duration. Every generated artifact stays
outside the build directory in the repository-root `log/` folder:

- `log/veyra-vs-mpv.json` — structured benchmark data log;
- `log/veyra-vs-mpv.png` — bar-chart summary generated from that JSON log;
- `log/veyra-vs-mpv-timeline.png` — line charts showing sampled CPU, RSS, and
  PSS over time;
- `log/logs/` — the captured stdout/stderr for each player run.

The white-background bar-chart summary has one panel per criterion. Each panel
compares the median across completed runs for Veyra and mpv, with the criterion,
statistic, unit, and “lower values are preferable” stated directly on the chart.

- **A. Startup latency** — process launch to first video output; unit: ms.
- **B. CPU utilisation** — total CPU time of all player threads during the
  sampling window; unit: % of one CPU core.
- **C. RSS memory** — per-run median resident set size during playback; unit:
  MiB.
- **D. PSS memory** — per-run median proportional set size during playback;
  unit: MiB. PSS apportions shared pages and is the more useful comparison
  measure.

The separate timeline PNG has line charts for interval CPU utilisation, RSS, and
PSS. Each line is the median at a sampling position across completed runs; its
shaded band spans the minimum to maximum. Startup is a one-time measurement, so
it is included only in the bar-chart summary. The benchmark records interval CPU
and memory samples at `--sample-interval` (250 ms by default).

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
python3 benchmarks/benchmark.py --mpv-hwdec=auto-safe
```

For a different fixture, ensure that `warmup + sample` is shorter than the
media duration:

```sh
python3 benchmarks/benchmark.py \
  --media /absolute/path/to/video.mp4 \
  --warmup-seconds 3 --sample-seconds 10 --runs 5
```

Run it on an otherwise idle desktop, use the same display/backend for both
players, and compare summary medians as well as the time-series shape rather
than a single run.
