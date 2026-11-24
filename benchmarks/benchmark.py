#!/usr/bin/env python3
"""Repeatable playback-FPS, CPU, memory and startup benchmarks for Veyra and mpv.

Measurement collection uses Python's standard library; PNG chart rendering uses
matplotlib. The tool is Linux-specific: CPU and memory come from /proc, and mpv
readiness is queried through its Unix IPC socket. The script never includes
build time in the reported startup time.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import importlib.util
import json
import math
import os
from pathlib import Path
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import time
from typing import Any, Callable


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MEDIA = REPOSITORY_ROOT / "video_test" / "test_02.mp4"
DEFAULT_VEYRA_BINARY = REPOSITORY_ROOT / "target" / "release" / "veyra"
DEFAULT_OUTPUT = REPOSITORY_ROOT / "log" / "veyra-vs-mpv.json"
PROC_STAT_CLOCK_TICKS = os.sysconf(os.sysconf_names["SC_CLK_TCK"])


class BenchmarkError(RuntimeError):
    """A benchmark could not reach a comparable, measurable playback state."""


@dataclass(frozen=True)
class MediaInfo:
    """Facts needed to keep media-clock estimates honest.

    A timeline expressed as frames/second is meaningful only for a controlled
    constant-frame-rate fixture.  For variable-rate content we retain duration
    but deliberately leave ``source_video_fps`` unset.
    """

    duration_seconds: float | None
    source_video_fps: float | None
    frame_rate_mode: str


PlaybackValue = float | int | bool | str
PlaybackSample = dict[str, PlaybackValue]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Measure Veyra and mpv startup, playback FPS, frame drops, CPU and RAM "
            "on one media file."
        )
    )
    parser.add_argument("--media", type=Path, default=DEFAULT_MEDIA, help="media fixture to play")
    parser.add_argument(
        "--veyra-bin",
        type=Path,
        default=DEFAULT_VEYRA_BINARY,
        help="release Veyra executable",
    )
    parser.add_argument("--mpv-bin", default="mpv", help="mpv executable or PATH name")
    parser.add_argument("--runs", type=positive_int, default=3, help="runs per player")
    parser.add_argument(
        "--warmup-seconds",
        type=non_negative_float,
        default=2.0,
        help="unmeasured playback time after the first frame",
    )
    parser.add_argument(
        "--sample-seconds",
        type=positive_float,
        default=5.0,
        help="length of the CPU/RAM measurement window",
    )
    parser.add_argument(
        "--sample-interval",
        type=positive_float,
        default=0.25,
        help="seconds between RAM samples",
    )
    parser.add_argument(
        "--ready-timeout",
        type=positive_float,
        default=20.0,
        help="maximum time to wait for first visible video output",
    )
    parser.add_argument(
        "--veyra-renderer",
        default="gpu-video",
        choices=("gpu-video", "gpu-native", "software", "vulkan-readback"),
        help="Veyra renderer passed through VEYRA_VIDEO_RENDERER",
    )
    parser.add_argument(
        "--veyra-threads",
        default="auto",
        help="VEYRA_VIDEO_THREADS value (auto, 1, or a positive thread count)",
    )
    parser.add_argument(
        "--veyra-thread-mode",
        default="auto",
        choices=("auto", "frame", "slice"),
        help="VEYRA_VIDEO_THREAD_MODE value",
    )
    parser.add_argument(
        "--mpv-hwdec",
        default="no",
        help="mpv --hwdec value; default no matches Veyra's software decode path",
    )
    parser.add_argument(
        "--mpv-video-sync",
        default="display-resample",
        help=(
            "mpv --video-sync value; display-resample enables mpv's display-synchronised "
            "FPS estimate when the video output supports it"
        ),
    )
    parser.add_argument(
        "--mpv-arg",
        action="append",
        default=[],
        help="extra mpv argument; may be supplied more than once",
    )
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT, help="JSON result path")
    parser.add_argument(
        "--chart-output",
        type=Path,
        help="PNG bar-chart path; defaults beside the JSON result",
    )
    parser.add_argument(
        "--timeline-output",
        type=Path,
        help="PNG time-series chart path; defaults beside the bar chart",
    )
    parser.add_argument(
        "--no-chart",
        action="store_true",
        help="write the JSON log only",
    )
    parser.add_argument(
        "--headless",
        action="store_true",
        help=(
            "mark this as a headless diagnostic run; renderer-backed RAM and physical-display "
            "ranking are reported as non-comparable"
        ),
    )
    parser.add_argument(
        "--build-veyra",
        action="store_true",
        help="build the release binary before benchmarking (excluded from measurements)",
    )
    return parser.parse_args()


def positive_int(value: str) -> int:
    result = int(value)
    if result <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return result


def positive_float(value: str) -> float:
    result = float(value)
    if result <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return result


def non_negative_float(value: str) -> float:
    result = float(value)
    if result < 0:
        raise argparse.ArgumentTypeError("must not be negative")
    return result


def resolve_executable(value: str | Path, label: str) -> Path:
    candidate = Path(value).expanduser()
    if candidate.is_file() and os.access(candidate, os.X_OK):
        return candidate.resolve()
    located = shutil.which(str(value))
    if located:
        return Path(located).resolve()
    raise BenchmarkError(f"{label} executable was not found: {value}")


def require_chart_renderer() -> None:
    if importlib.util.find_spec("matplotlib") is None:
        raise BenchmarkError(
            "matplotlib is required to create the PNG chart; install it with "
            "python3 -m pip install -r benchmarks/requirements.txt"
        )


def parse_frame_rate(value: object) -> float | None:
    """Parse ffprobe's ``numerator/denominator`` rate without accepting NaN."""
    if not isinstance(value, str) or "/" not in value:
        return None
    numerator, denominator = value.split("/", maxsplit=1)
    try:
        rate = int(numerator) / int(denominator)
    except (ValueError, ZeroDivisionError):
        return None
    return rate if math.isfinite(rate) and rate > 0 else None


def inspect_media(media: Path) -> MediaInfo:
    """Read duration plus a conservative CFR source-rate declaration once."""
    ffprobe = shutil.which("ffprobe")
    if not ffprobe:
        return MediaInfo(None, None, "unavailable")
    completed = subprocess.run(
        [
            ffprobe,
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "format=duration:stream=avg_frame_rate,r_frame_rate",
            "-of",
            "json",
            str(media),
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        return MediaInfo(None, None, "unavailable")
    try:
        document = json.loads(completed.stdout)
    except json.JSONDecodeError:
        return MediaInfo(None, None, "unavailable")
    duration: float | None = None
    raw_duration = document.get("format", {}).get("duration")
    try:
        candidate = float(raw_duration)
        if math.isfinite(candidate) and candidate >= 0:
            duration = candidate
    except (TypeError, ValueError):
        pass
    stream = next(iter(document.get("streams", ())), {})
    average_rate = parse_frame_rate(stream.get("avg_frame_rate"))
    real_rate = parse_frame_rate(stream.get("r_frame_rate"))
    if (
        average_rate is not None
        and real_rate is not None
        and math.isclose(average_rate, real_rate, rel_tol=1e-9, abs_tol=1e-9)
    ):
        return MediaInfo(duration, average_rate, "cfr")
    return MediaInfo(duration, None, "variable_or_unknown")


def read_proc_ticks(pid: int) -> int:
    try:
        content = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    except FileNotFoundError as error:
        raise BenchmarkError(f"process {pid} exited before CPU sampling") from error
    closing_paren = content.rfind(")")
    if closing_paren < 0:
        raise BenchmarkError(f"could not parse /proc/{pid}/stat")
    fields = content[closing_paren + 2 :].split()
    if len(fields) < 13:
        raise BenchmarkError(f"incomplete /proc/{pid}/stat")
    return int(fields[11]) + int(fields[12])


def read_memory_kib(pid: int) -> dict[str, int]:
    try:
        lines = Path(f"/proc/{pid}/smaps_rollup").read_text(encoding="utf-8").splitlines()
    except FileNotFoundError as error:
        raise BenchmarkError(f"process {pid} exited before RAM sampling") from error
    wanted = {
        "Rss:": "rss_kib",
        "Pss:": "pss_kib",
        "Pss_Anon:": "pss_anon_kib",
        "Pss_File:": "pss_file_kib",
        # GPU drivers often back textures and staging resources with memfd or
        # other shared mappings. Keeping this component visible prevents a
        # renderer/backend difference from being misdiagnosed as Rust heap use.
        "Pss_Shmem:": "pss_shmem_kib",
        "Private_Dirty:": "private_dirty_kib",
    }
    result: dict[str, int] = {}
    for line in lines:
        for prefix, key in wanted.items():
            if line.startswith(prefix):
                value = line[len(prefix) :].strip().split(maxsplit=1)
                if value:
                    result[key] = int(value[0])
                break
    if "rss_kib" not in result or "pss_kib" not in result:
        raise BenchmarkError(f"could not read RSS/PSS from /proc/{pid}/smaps_rollup")
    return result


def wait_while_running(process: subprocess.Popen[bytes], seconds: float, label: str) -> None:
    deadline = time.perf_counter() + seconds
    while True:
        if process.poll() is not None:
            raise BenchmarkError(f"{label} exited before the measurement finished")
        remaining = deadline - time.perf_counter()
        if remaining <= 0:
            return
        time.sleep(min(0.05, remaining))


def collect_metrics(
    process: subprocess.Popen[bytes],
    sample_seconds: float,
    interval: float,
    label: str,
    playback_sampler: Callable[[], PlaybackSample | None] | None = None,
) -> dict[str, Any]:
    started_at = time.perf_counter()
    started_ticks = read_proc_ticks(process.pid)
    previous_ticks = started_ticks
    previous_at = started_at
    deadline = started_at + sample_seconds
    samples: list[dict[str, float | int]] = []
    playback_samples: list[PlaybackSample] = []
    while True:
        if process.poll() is not None:
            raise BenchmarkError(f"{label} exited during CPU/RAM sampling")
        sampled_at = time.perf_counter()
        sampled_ticks = read_proc_ticks(process.pid)
        sample = read_memory_kib(process.pid)
        elapsed_seconds = round(sampled_at - started_at, 3)
        sample["elapsed_seconds"] = elapsed_seconds
        interval_seconds = sampled_at - previous_at
        if interval_seconds > 0.001:
            sample["cpu_percent"] = round(
                ((sampled_ticks - previous_ticks) / PROC_STAT_CLOCK_TICKS)
                / interval_seconds
                * 100.0,
                2,
            )
        samples.append(sample)
        if playback_sampler is not None:
            playback_sample = playback_sampler()
            if playback_sample is not None:
                playback_samples.append(
                    {"elapsed_seconds": elapsed_seconds, **playback_sample}
                )
        previous_at = sampled_at
        previous_ticks = sampled_ticks
        remaining = deadline - time.perf_counter()
        if remaining <= 0:
            break
        time.sleep(min(interval, remaining))
    finished_at = time.perf_counter()
    finished_ticks = read_proc_ticks(process.pid)
    elapsed = finished_at - started_at
    cpu_percent = ((finished_ticks - started_ticks) / PROC_STAT_CLOCK_TICKS) / elapsed * 100.0
    return {
        "duration_seconds": round(elapsed, 3),
        "cpu_percent": round(cpu_percent, 2),
        "memory_kib": summarize_memory(samples),
        "memory_samples_kib": samples,
        "playback_samples": playback_samples,
    }


def summarize_memory(samples: list[dict[str, float | int]]) -> dict[str, dict[str, int]]:
    result: dict[str, dict[str, int]] = {}
    for key in (
        "rss_kib",
        "pss_kib",
        "pss_anon_kib",
        "pss_file_kib",
        "pss_shmem_kib",
        "private_dirty_kib",
    ):
        values = [int(sample[key]) for sample in samples if key in sample]
        if values:
            result[key] = {
                "median": round(statistics.median(values)),
                "max": max(values),
            }
    return result


VEYRA_TELEMETRY_FIELDS = (
    "video_frames_processed",
    "rendered_frames",
    "playback_position_seconds",
    "render_fps",
    "frame_interval_ms",
    "frame_jitter_ms",
    "frame_processing_ms",
    "frame_processing_avg_ms",
    "frame_processing_peak_ms",
    "dropped_frames",
    "decode_threads",
)


def read_veyra_playback_metrics(path: Path) -> dict[str, float | int]:
    """Read the atomically-updated Veyra telemetry sidecar.

    The player writes this only when VEYRA_BENCHMARK_METRICS_FILE is set, so a
    missing or malformed file indicates the compared binary is too old or the
    benchmark session did not reach its sampling path.
    """
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        raise BenchmarkError("Veyra did not write playback telemetry") from error
    except json.JSONDecodeError as error:
        raise BenchmarkError(f"Veyra playback telemetry is invalid JSON: {error}") from error
    if not isinstance(document, dict) or document.get("schema_version") not in (1, 2):
        raise BenchmarkError("Veyra playback telemetry has an unsupported schema")
    metrics: dict[str, float | int] = {}
    for field in VEYRA_TELEMETRY_FIELDS:
        value = document.get(field)
        if value is None:
            continue
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise BenchmarkError(f"Veyra playback telemetry {field} is not numeric")
        metrics[field] = value
    required = {"video_frames_processed", "rendered_frames", "dropped_frames"}
    missing = required.difference(metrics)
    if missing:
        raise BenchmarkError(
            "Veyra playback telemetry is missing " + ", ".join(sorted(missing))
        )
    return metrics


def try_read_veyra_playback_metrics(path: Path) -> dict[str, float | int] | None:
    """Return no sample until Veyra has published its first atomic sidecar."""
    if not path.exists():
        return None
    return read_veyra_playback_metrics(path)


MPV_PLAYBACK_PROPERTIES = (
    "current-vo",
    "current-ao",
    "display-sync-active",
    "display-fps",
    "estimated-display-fps",
    "vsync-ratio",
    "estimated-vf-fps",
    "playback-time",
    "frame-drop-count",
    "decoder-frame-drop-count",
    "mistimed-frame-count",
    "vo-delayed-frame-count",
    "vo-presentation-delay",
)


def parse_mpv_ipc_responses(
    payload: bytes, requested_properties: dict[int, str]
) -> dict[str, Any]:
    """Extract matching IPC replies while ignoring asynchronous mpv events."""
    result: dict[str, Any] = {}
    for line in payload.splitlines():
        response = json.loads(line)
        request_id = response.get("request_id")
        property_name = requested_properties.get(request_id)
        if property_name is None:
            continue
        result[property_name] = (
            response.get("data") if response.get("error") == "success" else None
        )
    return result


def mpv_get_properties(socket_path: Path, properties: tuple[str, ...]) -> dict[str, Any]:
    """Read several mpv IPC properties, matching responses by request ID."""
    if not socket_path.exists():
        raise FileNotFoundError(socket_path)
    requested_properties = {
        8100 + index: property_name for index, property_name in enumerate(properties)
    }
    request = b"".join(
        (
            json.dumps(
                {"command": ["get_property", property_name], "request_id": request_id}
            ).encode("utf-8")
            + b"\n"
        )
        for request_id, property_name in requested_properties.items()
    )
    received = b""
    matched: dict[str, Any] = {}
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(0.25)
        connection.connect(str(socket_path))
        connection.sendall(request)
        while len(matched) < len(requested_properties):
            chunk = connection.recv(4096)
            if not chunk:
                break
            received += chunk
            complete, separator, remainder = received.rpartition(b"\n")
            if not separator:
                continue
            received = remainder
            matched.update(parse_mpv_ipc_responses(complete, requested_properties))
    return matched


def finite_number(value: object) -> float | int | None:
    """Return a numeric IPC value only if it is finite and non-boolean."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    return value if math.isfinite(value) else None


def read_mpv_playback_metrics(socket_path: Path) -> dict[str, float | int | bool | str]:
    """Read mpv clocks, display diagnostics, and cumulative frame counters."""
    values = mpv_get_properties(socket_path, MPV_PLAYBACK_PROPERTIES)
    metrics: dict[str, float | int | bool | str] = {}
    boolean_fields = {"display-sync-active": "display_sync_active"}
    numeric_fields = {
        "display-fps": "display_refresh_fps",
        "estimated-display-fps": "estimated_display_fps",
        "vsync-ratio": "vsync_ratio",
        "estimated-vf-fps": "estimated_vf_fps",
        "playback-time": "playback_time_seconds",
        "frame-drop-count": "vo_frame_drop_count",
        "decoder-frame-drop-count": "decoder_frame_drop_count",
        "mistimed-frame-count": "mistimed_frame_count",
        "vo-delayed-frame-count": "vo_delayed_frame_count",
        "vo-presentation-delay": "vo_presentation_delay_seconds",
    }
    string_fields = {
        "current-vo": "current_vo",
        "current-ao": "current_ao",
    }
    for property_name, metric_name in boolean_fields.items():
        value = values.get(property_name)
        if isinstance(value, bool):
            metrics[metric_name] = value
    for property_name, metric_name in numeric_fields.items():
        if (value := finite_number(values.get(property_name))) is not None:
            metrics[metric_name] = value
    for property_name, metric_name in string_fields.items():
        value = values.get(property_name)
        if isinstance(value, str) and value:
            metrics[metric_name] = value

    missing = {"vo_frame_drop_count", "decoder_frame_drop_count"}.difference(metrics)
    if missing:
        raise BenchmarkError("mpv IPC is missing " + ", ".join(sorted(missing)))
    return metrics


def mpv_display_sync_scheduled_cadence_fps(
    sample: dict[str, float | int | bool | str],
) -> float | None:
    """Return mpv's schedule cadence, never a claimed presentation rate.

    ``vsync-ratio`` includes zero-vsync dropped frames, so a cadence can exceed
    physical refresh.  Retain it as a diagnostic only; it is not a display FPS.
    """
    if sample.get("display_sync_active") is not True:
        return None
    display_fps = sample.get("estimated_display_fps")
    vsync_ratio = sample.get("vsync_ratio")
    if (
        isinstance(display_fps, bool)
        or not isinstance(display_fps, (int, float))
        or isinstance(vsync_ratio, bool)
        or not isinstance(vsync_ratio, (int, float))
        or display_fps <= 0
        or vsync_ratio <= 0
    ):
        return None
    return float(display_fps) / float(vsync_ratio)


def append_final_playback_sample(
    metrics: dict[str, Any], playback_sample: PlaybackSample
) -> None:
    """Append an end snapshot without creating a near-zero timeline interval."""
    final_sample = {
        "elapsed_seconds": metrics["duration_seconds"],
        **playback_sample,
        "snapshot_only": True,
    }
    # Keep the last periodic sample as well: a very short measurement window
    # may otherwise have only one sample and be unable to form a counter delta.
    # Timeline annotations deliberately skip this snapshot-only point.
    metrics["playback_samples"].append(final_sample)


def counter_delta(
    samples: list[PlaybackSample], field: str
) -> tuple[float, float]:
    """Return a monotonic counter delta and its elapsed measurement time."""
    usable = [
        sample
        for sample in samples
        if finite_number(sample.get(field)) is not None
        and finite_number(sample.get("elapsed_seconds")) is not None
    ]
    if len(usable) < 2:
        raise BenchmarkError(f"not enough {field} samples to calculate playback FPS")
    first, last = usable[0], usable[-1]
    elapsed = float(finite_number(last["elapsed_seconds"])) - float(
        finite_number(first["elapsed_seconds"])
    )
    delta = float(finite_number(last[field])) - float(finite_number(first[field]))
    if elapsed <= 0 or delta < 0:
        raise BenchmarkError(f"invalid {field} counter progression during playback sampling")
    return delta, elapsed


def annotate_media_clock_samples(
    samples: list[PlaybackSample],
    position_field: str,
    source_video_fps: float | None,
) -> None:
    """Add a CFR timeline-rate estimate without calling it presentation FPS."""
    if source_video_fps is None:
        return
    previous: PlaybackSample | None = None
    for sample in samples:
        if sample.get("snapshot_only") is True:
            previous = sample
            continue
        if previous is not None:
            elapsed_value = finite_number(sample.get("elapsed_seconds"))
            previous_elapsed_value = finite_number(previous.get("elapsed_seconds"))
            position = finite_number(sample.get(position_field))
            previous_position = finite_number(previous.get(position_field))
            if (
                elapsed_value is not None
                and previous_elapsed_value is not None
                and position is not None
                and previous_position is not None
            ):
                elapsed = float(elapsed_value) - float(previous_elapsed_value)
                media_delta = float(position) - float(previous_position)
                if elapsed > 0 and media_delta >= 0:
                    speed = media_delta / elapsed
                    sample["media_clock_speed"] = round(speed, 4)
                    sample["media_clock_fps_estimate"] = round(source_video_fps * speed, 2)
        previous = sample


def summarize_media_clock(
    samples: list[PlaybackSample], position_field: str, source_video_fps: float | None
) -> dict[str, float]:
    """Use full-window media-time progress so 250 ms sampling jitter cancels out."""
    if source_video_fps is None:
        return {}
    try:
        media_delta, elapsed = counter_delta(samples, position_field)
    except BenchmarkError:
        return {}
    speed = media_delta / elapsed
    return {
        "media_clock_speed": round(speed, 4),
        "media_clock_fps_estimate": round(source_video_fps * speed, 2),
    }


def annotate_veyra_playback_samples(
    samples: list[PlaybackSample], source_video_fps: float | None = None
) -> None:
    previous: PlaybackSample | None = None
    for sample in samples:
        if sample.get("snapshot_only") is True:
            previous = sample
            continue
        if previous is not None:
            elapsed = float(sample["elapsed_seconds"]) - float(previous["elapsed_seconds"])
            if elapsed > 0:
                for source, target in (
                    ("rendered_frames", "delivery_fps"),
                    ("video_frames_processed", "processed_fps"),
                    ("dropped_frames", "frame_drop_fps"),
                ):
                    delta = float(sample[source]) - float(previous[source])
                    if delta >= 0:
                        sample[target] = round(delta / elapsed, 2)
        previous = sample
    annotate_media_clock_samples(samples, "playback_position_seconds", source_video_fps)


def summarize_veyra_playback(
    samples: list[PlaybackSample], source_video_fps: float | None = None
) -> dict[str, Any]:
    rendered_delta, elapsed = counter_delta(samples, "rendered_frames")
    processed_delta, _ = counter_delta(samples, "video_frames_processed")
    dropped_delta, _ = counter_delta(samples, "dropped_frames")
    final = samples[-1]
    result: dict[str, Any] = {
        "delivery_fps_measurement": "native_surface_handoff",
        "measurement_window_seconds": round(elapsed, 3),
        "delivery_fps": round(rendered_delta / elapsed, 2),
        "processed_fps": round(processed_delta / elapsed, 2),
        "frame_drop_count": round(dropped_delta),
        "frame_drop_fps": round(dropped_delta / elapsed, 2),
        "frame_drop_semantics": "processed_minus_delivered_estimate",
    }
    result.update(summarize_media_clock(samples, "playback_position_seconds", source_video_fps))
    for source, target in (
        ("render_fps", "render_fps_rolling"),
        ("frame_interval_ms", "frame_interval_ms"),
        ("frame_jitter_ms", "frame_jitter_ms"),
        ("frame_processing_ms", "frame_processing_ms"),
        ("frame_processing_avg_ms", "frame_processing_avg_ms"),
        ("frame_processing_peak_ms", "frame_processing_peak_ms"),
        ("decode_threads", "decode_threads"),
    ):
        if source in final:
            result[target] = final[source]
    return result


def annotate_mpv_playback_samples(
    samples: list[PlaybackSample], source_video_fps: float | None = None
) -> None:
    for sample in samples:
        if sample.get("snapshot_only") is True:
            continue
        scheduled_fps = mpv_display_sync_scheduled_cadence_fps(sample)
        if scheduled_fps is not None:
            sample["display_sync_scheduled_cadence_fps"] = round(scheduled_fps, 2)
    annotate_media_clock_samples(samples, "playback_time_seconds", source_video_fps)


def summarize_mpv_playback(
    samples: list[PlaybackSample], source_video_fps: float | None = None
) -> dict[str, Any]:
    vo_drops, elapsed = counter_delta(samples, "vo_frame_drop_count")
    decoder_drops, _ = counter_delta(samples, "decoder_frame_drop_count")
    result: dict[str, Any] = {
        "measurement_window_seconds": round(elapsed, 3),
        "vo_frame_drop_count": round(vo_drops),
        "decoder_frame_drop_count": round(decoder_drops),
        "vo_frame_drop_fps": round(vo_drops / elapsed, 2),
        "decoder_frame_drop_fps": round(decoder_drops / elapsed, 2),
    }
    media_clock = summarize_media_clock(samples, "playback_time_seconds", source_video_fps)
    result.update(media_clock)
    if (media_clock_fps := media_clock.get("media_clock_fps_estimate")) is not None:
        result["source_frame_admission_fps_estimate"] = round(
            max(0.0, media_clock_fps - (vo_drops + decoder_drops) / elapsed), 2
        )
        result["source_frame_admission_semantics"] = (
            "cfr_media_clock_minus_vo_and_decoder_drops"
        )
    for source, target in (
        ("mistimed_frame_count", "mistimed_frame_count"),
        ("vo_delayed_frame_count", "vo_delayed_frame_count"),
    ):
        try:
            delta, _ = counter_delta(samples, source)
        except BenchmarkError:
            continue
        result[target] = round(delta)
    scheduled_fps_values = [
        float(sample["display_sync_scheduled_cadence_fps"])
        for sample in samples
        if finite_number(sample.get("display_sync_scheduled_cadence_fps")) is not None
    ]
    if scheduled_fps_values:
        result["display_sync_scheduled_cadence_fps"] = round(
            statistics.median(scheduled_fps_values), 2
        )
    final = samples[-1]
    for source, target in (
        ("display_refresh_fps", "display_refresh_estimate_fps"),
        ("estimated_display_fps", "estimated_display_fps"),
        ("vsync_ratio", "vsync_ratio"),
        ("vo_presentation_delay_seconds", "vo_presentation_delay_seconds"),
    ):
        if source in final:
            result[target] = final[source]
    for source, target in (("current_vo", "current_vo"), ("current_ao", "current_ao")):
        if isinstance(final.get(source), str):
            result[target] = final[source]
    if final.get("display_sync_active") is not True:
        result["display_sync_unavailable_reason"] = "no_display_sync"
    return result


def spawn_process(
    command: list[str], env: dict[str, str], log_path: Path
) -> subprocess.Popen[bytes]:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("wb") as log:
        return subprocess.Popen(
            command,
            cwd=REPOSITORY_ROOT,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )


def stop_process(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    try:
        if os.name == "posix":
            os.killpg(process.pid, signal.SIGTERM)
        else:
            process.terminate()
        process.wait(timeout=3)
    except ProcessLookupError:
        return
    except subprocess.TimeoutExpired:
        try:
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
        except ProcessLookupError:
            return
        process.wait(timeout=3)


def wait_for_ready(
    process: subprocess.Popen[bytes],
    timeout: float,
    ready: Callable[[], bool],
    label: str,
    started_at: float,
) -> float:
    deadline = started_at + timeout
    while time.perf_counter() < deadline:
        if process.poll() is not None:
            raise BenchmarkError(f"{label} exited before it presented a video frame")
        try:
            if ready():
                return (time.perf_counter() - started_at) * 1000.0
        except (ConnectionError, FileNotFoundError, OSError, json.JSONDecodeError):
            pass
        time.sleep(0.01)
    raise BenchmarkError(f"{label} did not present a video frame within {timeout:.1f} seconds")


def mpv_is_ready(socket_path: Path) -> bool:
    return bool(mpv_get_properties(socket_path, ("vo-configured",)).get("vo-configured"))


def run_veyra(
    args: argparse.Namespace, run_number: int, temporary_dir: Path, logs_dir: Path
) -> dict[str, Any]:
    ready_path = temporary_dir / f"veyra-ready-{run_number}"
    playback_metrics_path = temporary_dir / f"veyra-playback-{run_number}.json"
    ready_path.unlink(missing_ok=True)
    playback_metrics_path.unlink(missing_ok=True)
    environment = os.environ.copy()
    environment.update(
        {
            "VEYRA_OPEN_MEDIA": str(args.media),
            "VEYRA_VIDEO_RENDERER": args.veyra_renderer,
            "VEYRA_VIDEO_THREADS": args.veyra_threads,
            "VEYRA_VIDEO_THREAD_MODE": args.veyra_thread_mode,
            "VEYRA_BENCHMARK_READY_FILE": str(ready_path),
            "VEYRA_BENCHMARK_METRICS_FILE": str(playback_metrics_path),
        }
    )
    command = [str(args.veyra_bin)]
    log_path = logs_dir / f"veyra-run-{run_number}.log"
    started_at = time.perf_counter()
    process = spawn_process(command, environment, log_path)
    try:
        startup_ms = wait_for_ready(
            process, args.ready_timeout, ready_path.exists, "Veyra", started_at
        )
        wait_while_running(process, args.warmup_seconds, "Veyra")
        metrics = collect_metrics(
            process,
            args.sample_seconds,
            args.sample_interval,
            "Veyra",
            lambda: try_read_veyra_playback_metrics(playback_metrics_path),
        )
        append_final_playback_sample(
            metrics, read_veyra_playback_metrics(playback_metrics_path)
        )
        annotate_veyra_playback_samples(metrics["playback_samples"], args.source_video_fps)
        playback_metrics = summarize_veyra_playback(
            metrics["playback_samples"], args.source_video_fps
        )
    finally:
        stop_process(process)
    return {
        "player": "veyra",
        "run": run_number,
        "command": command,
        "startup_ms": round(startup_ms, 2),
        "metrics": metrics,
        "playback_metrics": playback_metrics,
        "log": str(log_path),
    }


def run_mpv(
    args: argparse.Namespace, run_number: int, temporary_dir: Path, logs_dir: Path
) -> dict[str, Any]:
    socket_path = temporary_dir / f"mpv-{run_number}.sock"
    socket_path.unlink(missing_ok=True)
    command = [
        str(args.mpv_bin),
        "--no-config",
        "--force-window=yes",
        "--keep-open=yes",
        f"--hwdec={args.mpv_hwdec}",
        f"--video-sync={args.mpv_video_sync}",
        "--framedrop=vo",
        f"--input-ipc-server={socket_path}",
        *args.mpv_arg,
        str(args.media),
    ]
    log_path = logs_dir / f"mpv-run-{run_number}.log"
    started_at = time.perf_counter()
    process = spawn_process(command, os.environ.copy(), log_path)
    try:
        startup_ms = wait_for_ready(
            process,
            args.ready_timeout,
            lambda: mpv_is_ready(socket_path),
            "mpv",
            started_at,
        )
        wait_while_running(process, args.warmup_seconds, "mpv")
        def sample_mpv_playback() -> PlaybackSample | None:
            try:
                return read_mpv_playback_metrics(socket_path)
            except (ConnectionError, FileNotFoundError, OSError, json.JSONDecodeError):
                return None

        metrics = collect_metrics(
            process,
            args.sample_seconds,
            args.sample_interval,
            "mpv",
            sample_mpv_playback,
        )
        append_final_playback_sample(metrics, read_mpv_playback_metrics(socket_path))
        annotate_mpv_playback_samples(metrics["playback_samples"], args.source_video_fps)
        playback_metrics = summarize_mpv_playback(
            metrics["playback_samples"], args.source_video_fps
        )
    finally:
        stop_process(process)
        socket_path.unlink(missing_ok=True)
    return {
        "player": "mpv",
        "run": run_number,
        "command": command,
        "startup_ms": round(startup_ms, 2),
        "metrics": metrics,
        "playback_metrics": playback_metrics,
        "log": str(log_path),
    }


def distribution(values: list[float]) -> dict[str, float]:
    return {"median": round(statistics.median(values), 2), "max": round(max(values), 2)}


def summarize_playback_metrics(runs: list[dict[str, Any]]) -> dict[str, dict[str, float]]:
    summary: dict[str, dict[str, float]] = {}
    fields = {
        field
        for run in runs
        for field, value in run.get("playback_metrics", {}).items()
        if isinstance(value, (int, float)) and not isinstance(value, bool)
    }
    for field in sorted(fields):
        values = [
            float(run["playback_metrics"][field])
            for run in runs
            if isinstance(run.get("playback_metrics", {}).get(field), (int, float))
            and not isinstance(run.get("playback_metrics", {}).get(field), bool)
        ]
        if values:
            summary[field] = distribution(values)
    return summary


def summarize_runs(runs: list[dict[str, Any]]) -> dict[str, Any]:
    summary: dict[str, Any] = {
        "startup_ms": distribution([run["startup_ms"] for run in runs]),
        "cpu_percent": distribution([run["metrics"]["cpu_percent"] for run in runs]),
        "memory_kib": {},
    }
    for key in (
        "rss_kib",
        "pss_kib",
        "pss_anon_kib",
        "pss_file_kib",
        "pss_shmem_kib",
        "private_dirty_kib",
    ):
        values = [run["metrics"]["memory_kib"].get(key) for run in runs]
        medians = [value["median"] for value in values if value is not None]
        maxima = [value["max"] for value in values if value is not None]
        if not medians:
            continue
        summary["memory_kib"][key] = {
            "median_of_run_medians": round(statistics.median(medians)),
            "max_of_runs": max(maxima),
        }
    playback = summarize_playback_metrics(runs)
    if playback:
        summary["playback_metrics"] = playback
    return summary


def format_mib(kib: float) -> str:
    return f"{kib / 1024.0:.1f} MiB"


def print_summary(summaries: dict[str, dict[str, Any]], *, headless: bool) -> None:
    def memory_value(memory: dict[str, Any], name: str) -> str:
        metric = memory.get(name)
        if metric is None:
            return "N/A"
        return f"{format_mib(metric['median_of_run_medians'])} / {format_mib(metric['max_of_runs'])}"

    def median(playback: dict[str, Any], name: str) -> str:
        metric = playback.get(name)
        return "N/A" if metric is None else f"{metric['median']:.2f}"

    print()
    print("player  PSS primary (median / max)  PSS shared (median / max)  RSS (median / max)")
    for player in ("veyra", "mpv"):
        memory = summaries[player]["memory_kib"]
        print(
            f"{player:<6}  {memory_value(memory, 'pss_kib'):<27}  "
            f"{memory_value(memory, 'pss_shmem_kib'):<26}  "
            f"{memory_value(memory, 'rss_kib')}"
        )
    print("PSS is the primary cross-process footprint; RSS is supplemental.")
    print()
    print("player  startup (median)  CPU (median)")
    for player in ("veyra", "mpv"):
        summary = summaries[player]
        print(
            f"{player:<6}  {summary['startup_ms']['median']:>8.2f} ms      "
            f"{summary['cpu_percent']['median']:>6.2f}%"
        )
    print()
    print("player  CFR media-clock FPS  handoff / admission estimate       drop counters")
    for player in ("veyra", "mpv"):
        playback = summaries[player].get("playback_metrics", {})
        if player == "veyra":
            pipeline_rate = f"NativeSurface handoff {median(playback, 'delivery_fps')}"
            drops = f"undelivered {median(playback, 'frame_drop_count')}"
        else:
            pipeline_rate = f"source admission est. {median(playback, 'source_frame_admission_fps_estimate')}"
            drops = (
                f"VO {median(playback, 'vo_frame_drop_count')}, "
                f"decoder {median(playback, 'decoder_frame_drop_count')}"
            )
        print(
            f"{player:<6}  {median(playback, 'media_clock_fps_estimate'):>19}  "
            f"{pipeline_rate:<34}  {drops}"
        )
    mpv_playback = summaries["mpv"].get("playback_metrics", {})
    scheduled = median(mpv_playback, "display_sync_scheduled_cadence_fps")
    refresh = median(mpv_playback, "display_refresh_estimate_fps")
    print(
        "CFR media-clock FPS = nominal source FPS × media-time progress / wall time; "
        "it is not a physical presentation counter."
    )
    print(
        "mpv display diagnostics: refresh estimate "
        f"{refresh}, scheduled cadence {scheduled} (diagnostic only; never presentation FPS)."
    )
    if headless:
        print(
            "HEADLESS DIAGNOSTIC: do not rank player RAM or physical-display FPS here; "
            "Xvfb/Mesa can select different renderer backends and driver allocations."
        )


def write_json(path: Path, document: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def main() -> int:
    args = parse_args()
    if not Path("/proc").is_dir() or not hasattr(socket, "AF_UNIX"):
        raise BenchmarkError("this benchmark currently requires Linux /proc and Unix sockets")
    if not args.no_chart:
        require_chart_renderer()
    args.media = args.media.expanduser().resolve()
    if not args.media.is_file():
        raise BenchmarkError(f"media file does not exist: {args.media}")
    if args.build_veyra:
        subprocess.run(["cargo", "build", "--release"], cwd=REPOSITORY_ROOT, check=True)
    args.veyra_bin = resolve_executable(args.veyra_bin, "Veyra")
    args.mpv_bin = resolve_executable(args.mpv_bin, "mpv")
    media_info = inspect_media(args.media)
    args.source_video_fps = media_info.source_video_fps
    duration = media_info.duration_seconds
    playback_window = args.warmup_seconds + args.sample_seconds
    if duration is not None and playback_window >= duration:
        raise BenchmarkError(
            f"warmup + sample is {playback_window:.1f}s but the media is only {duration:.1f}s; "
            "choose a shorter window or a longer fixture"
        )

    args.output = args.output.expanduser().resolve()
    logs_dir = args.output.parent / "logs"
    args.chart_output = (
        args.chart_output.expanduser().resolve()
        if args.chart_output
        else args.output.with_suffix(".png")
    )
    args.timeline_output = (
        args.timeline_output.expanduser().resolve()
        if args.timeline_output
        else args.chart_output.with_name(f"{args.chart_output.stem}-timeline.png")
    )
    run_results: dict[str, list[dict[str, Any]]] = {"veyra": [], "mpv": []}
    with tempfile.TemporaryDirectory(prefix="veyra-mpv-benchmark-") as temporary:
        temporary_dir = Path(temporary)
        for run_number in range(1, args.runs + 1):
            # Alternate first player to reduce systematic cache/thermal bias.
            order = ("veyra", "mpv") if run_number % 2 else ("mpv", "veyra")
            for player in order:
                print(f"run {run_number}/{args.runs}: {player}", flush=True)
                runner = run_veyra if player == "veyra" else run_mpv
                run_results[player].append(runner(args, run_number, temporary_dir, logs_dir))

    summaries = {player: summarize_runs(runs) for player, runs in run_results.items()}
    document = {
        "schema_version": 5,
        "generated_at_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "environment": {
            "platform": sys.platform,
            "kernel": os.uname().release,
            "cpu_count": os.cpu_count(),
            "clock_ticks_per_second": PROC_STAT_CLOCK_TICKS,
            "python": sys.version.split()[0],
        },
        "media": {
            "path": str(args.media),
            "duration_seconds": duration,
            "source_video_fps": media_info.source_video_fps,
            "frame_rate_mode": media_info.frame_rate_mode,
        },
        "configuration": {
            "runs": args.runs,
            "warmup_seconds": args.warmup_seconds,
            "sample_seconds": args.sample_seconds,
            "sample_interval_seconds": args.sample_interval,
            "veyra_renderer": args.veyra_renderer,
            "veyra_window_backend": os.environ.get("WINIT_UNIX_BACKEND", "auto"),
            "veyra_threads": args.veyra_threads,
            "veyra_thread_mode": args.veyra_thread_mode,
            "mpv_hwdec": args.mpv_hwdec,
            "mpv_video_sync": args.mpv_video_sync,
            "execution_mode": "headless_diagnostic" if args.headless else "desktop_end_to_end",
        },
        "comparison_scope": {
            "ram_ranking": "diagnostic_only" if args.headless else "end_to_end_footprint",
            "display_presentation_fps": "unavailable_in_headless" if args.headless else "backend_dependent",
        },
        "chart": None if args.no_chart else str(args.chart_output),
        "timeline_chart": None if args.no_chart else str(args.timeline_output),
        "runs": run_results,
        "summary": summaries,
    }
    write_json(args.output, document)
    if not args.no_chart:
        subprocess.run(
            [
                sys.executable,
                str(Path(__file__).with_name("render_chart.py")),
                "--input",
                str(args.output),
                "--output",
                str(args.chart_output),
                "--timeline-output",
                str(args.timeline_output),
            ],
            check=True,
        )
    print_summary(summaries, headless=args.headless)
    print(f"\nJSON: {args.output}")
    if not args.no_chart:
        print(f"Bar charts: {args.chart_output}")
        print(f"Timeline charts: {args.timeline_output}")
    print(f"Logs: {logs_dir}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (BenchmarkError, subprocess.SubprocessError) as error:
        print(f"benchmark failed: {error}", file=sys.stderr)
        raise SystemExit(2)
