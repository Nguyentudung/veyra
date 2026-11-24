#!/usr/bin/env python3
"""Render bar summaries and time-series PNGs from a Veyra/mpv benchmark log."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import statistics
import tempfile
from typing import Any, Callable

if "MPLCONFIGDIR" not in os.environ:
    matplotlib_cache = Path(tempfile.gettempdir()) / "veyra-matplotlib-cache"
    matplotlib_cache.mkdir(parents=True, exist_ok=True)
    os.environ["MPLCONFIGDIR"] = str(matplotlib_cache)

try:
    import matplotlib

    matplotlib.use("Agg")
    from matplotlib import pyplot as plt
    from matplotlib.lines import Line2D
except ImportError as error:
    raise SystemExit(
        "matplotlib is required to render PNG charts. Install it with: "
        "python3 -m pip install -r benchmarks/requirements.txt"
    ) from error


VEYRA = "#2C6DB2"
MPV = "#C75B12"
TEXT = "#1F2937"
MUTED = "#4B5563"
GRID = "#D1D5DB"
PANEL = "#FCFCFC"
PLAYERS = ("veyra", "mpv")
PLAYER_LABELS = {"veyra": "Veyra", "mpv": "mpv"}
PLAYER_COLORS = {"veyra": VEYRA, "mpv": MPV}


@dataclass(frozen=True)
class BarMetric:
    section: str
    title: str
    criterion: str
    axis_label: str
    value_from_run: Callable[[dict[str, Any]], float]
    higher_is_better: bool = False
    allow_missing: bool = False


@dataclass(frozen=True)
class TimeMetric:
    title: str
    criterion: str
    axis_label: str
    sample_collection: str
    sample_key: str
    transform: Callable[[float], float]


BAR_METRICS = (
    BarMetric(
        "A",
        "Độ trễ khởi động",
        "Từ lúc khởi chạy tiến trình đến khung hình video đầu tiên được hiển thị",
        "Thời gian khởi động (ms)",
        lambda run: float(run["startup_ms"]),
    ),
    BarMetric(
        "B",
        "Mức sử dụng CPU",
        "Tổng thời gian CPU của tiến trình trong cửa sổ đo (toàn bộ luồng)",
        "Mức sử dụng CPU (% của một lõi)",
        lambda run: float(run["metrics"]["cpu_percent"]),
    ),
    BarMetric(
        "C",
        "Bộ nhớ PSS (chính)",
        "PSS trung vị của từng lượt trong khi phát; đây là footprint chính để so sánh tiến trình",
        "PSS trung vị mỗi lượt (MiB)",
        lambda run: float(run["metrics"]["memory_kib"]["pss_kib"]["median"]) / 1024.0,
    ),
    BarMetric(
        "D",
        "PSS bộ nhớ chia sẻ / GPU",
        "Pss_Shmem trung vị; giúp nhận diện memfd/driver/texture không phải heap ứng dụng",
        "PSS shared trung vị mỗi lượt (MiB)",
        lambda run: float(run["metrics"]["memory_kib"]["pss_shmem_kib"]["median"]) / 1024.0,
        allow_missing=True,
    ),
    BarMetric(
        "E",
        "Bộ nhớ RSS (phụ)",
        "RSS trung vị của từng lượt trong khi phát; bao gồm trang dùng chung đầy đủ",
        "RSS trung vị mỗi lượt (MiB)",
        lambda run: float(run["metrics"]["memory_kib"]["rss_kib"]["median"]) / 1024.0,
    ),
    BarMetric(
        "F",
        "FPS theo media clock (CFR)",
        (
            "FPS danh nghĩa của fixture CFR nhân với tốc độ tiến media / thời gian thực. "
            "Có thể đo dưới Xvfb nhưng không phải số khung physically presented."
        ),
        "FPS ước lượng theo media clock",
        lambda run: float(run["playback_metrics"]["media_clock_fps_estimate"]),
        higher_is_better=True,
        allow_missing=True,
    ),
)

TIME_METRICS = (
    TimeMetric(
        "Diễn biến mức sử dụng CPU",
        "CPU của tiến trình theo từng khoảng trong cửa sổ đo",
        "Mức sử dụng CPU (% của một lõi)",
        "memory_samples_kib",
        "cpu_percent",
        lambda value: value,
    ),
    TimeMetric(
        "Diễn biến bộ nhớ RSS",
        "RSS được lấy mẫu trong khi phát",
        "Bộ nhớ RSS (MiB)",
        "memory_samples_kib",
        "rss_kib",
        lambda value: value / 1024.0,
    ),
    TimeMetric(
        "Diễn biến bộ nhớ PSS",
        "PSS được lấy mẫu trong khi phát",
        "Bộ nhớ PSS (MiB)",
        "memory_samples_kib",
        "pss_kib",
        lambda value: value / 1024.0,
    ),
    TimeMetric(
        "Diễn biến PSS bộ nhớ chia sẻ / GPU",
        "Pss_Shmem lấy mẫu trong khi phát; làm rõ chi phí memfd/driver/texture",
        "PSS shared (MiB)",
        "memory_samples_kib",
        "pss_shmem_kib",
        lambda value: value / 1024.0,
    ),
    TimeMetric(
        "Diễn biến FPS theo media clock (CFR)",
        (
            "FPS danh nghĩa của fixture CFR nhân với tiến độ media / thời gian thực. "
            "Có cả khi mpv không có display-sync, nhưng không phải số khung đã present."
        ),
        "FPS ước lượng theo media clock",
        "playback_samples",
        "media_clock_fps_estimate",
        lambda value: value,
    ),
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Render Veyra/mpv bar and time-series PNG charts from JSON."
    )
    parser.add_argument("--input", required=True, type=Path, help="benchmark JSON log")
    parser.add_argument("--output", required=True, type=Path, help="summary bar-chart PNG path")
    parser.add_argument("--timeline-output", type=Path, help="time-series PNG path")
    return parser.parse_args()


def run_values(document: dict[str, Any], player: str, metric: BarMetric) -> list[float]:
    try:
        runs = document["runs"][player]
    except KeyError as error:
        raise ValueError(f"benchmark JSON is missing runs for {player}") from error
    if not runs:
        raise ValueError(f"benchmark JSON has no completed runs for {player}")
    values: list[float] = []
    try:
        for run in runs:
            value = metric.value_from_run(run)
            if isinstance(value, bool) or not isinstance(value, (int, float)):
                raise TypeError("value is not numeric")
            values.append(float(value))
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError(
            f"benchmark JSON has no usable {metric.title.lower()} data for {player}"
        ) from error
    return values


def style_axis(axis: Any) -> None:
    axis.set_facecolor(PANEL)
    axis.grid(axis="y", color=GRID, linewidth=0.8)
    axis.set_axisbelow(True)
    for spine in axis.spines.values():
        spine.set_color(GRID)
        spine.set_linewidth(0.9)
    axis.tick_params(axis="x", colors=TEXT, labelsize=10)
    axis.tick_params(axis="y", colors=MUTED, labelsize=9)


def protocol_text(document: dict[str, Any]) -> str:
    configuration = document.get("configuration", {})
    media = Path(document.get("media", {}).get("path", "unknown media")).name
    execution_mode = configuration.get("execution_mode", "desktop_end_to_end")
    scope = (
        "HEADLESS diagnostic: không xếp hạng RAM/FPS trình bày giữa hai player."
        if execution_mode == "headless_diagnostic"
        else "Desktop end-to-end: vẫn cần cùng backend/VO/AO để so sánh nghiêm ngặt."
    )
    return (
        f"Video: {media}  |  Lượt chạy: Veyra n={len(document['runs']['veyra'])}, "
        f"mpv n={len(document['runs']['mpv'])}  |  "
        f"Làm nóng: {configuration.get('warmup_seconds', 'không rõ')} giây  |  "
        f"Cửa sổ đo: {configuration.get('sample_seconds', 'không rõ')} giây  |  "
        f"Chu kỳ lấy mẫu: {configuration.get('sample_interval_seconds', 'không rõ')} giây\n"
        f"Trình dựng Veyra: {configuration.get('veyra_renderer', 'không rõ')}  |  "
        f"Backend cửa sổ Veyra: {configuration.get('veyra_window_backend', 'auto')}  |  "
        f"Giải mã phần cứng mpv: {configuration.get('mpv_hwdec', 'không rõ')}  |  "
        f"Đồng bộ video mpv: {configuration.get('mpv_video_sync', 'không rõ')}\n"
        f"Chế độ: {execution_mode}. {scope}"
    )


def draw_bar_metric(axis: Any, document: dict[str, Any], metric: BarMetric) -> None:
    values: list[float | None] = []
    for player in PLAYERS:
        try:
            values.append(statistics.median(run_values(document, player, metric)))
        except ValueError:
            if not metric.allow_missing:
                raise
            values.append(None)
    visible_values = [value for value in values if value is not None]
    bar_values = [value if value is not None else 0.0 for value in values]
    bars = axis.bar(
        [PLAYER_LABELS[player] for player in PLAYERS],
        bar_values,
        color=[PLAYER_COLORS[player] for player in PLAYERS],
        width=0.56,
    )
    for bar, value in zip(bars, values, strict=True):
        if value is None:
            bar.set_hatch("//")
            bar.set_alpha(0.45)
    maximum = max(visible_values) if visible_values else 0.0
    axis.set_ylim(0, maximum * 1.22 if maximum > 0 else 1.0)
    axis.set_ylabel(metric.axis_label, color=TEXT, fontsize=10)
    axis.bar_label(
        bars,
        labels=["N/A" if value is None else f"{value:.2f}" for value in values],
        padding=5,
        color=TEXT,
        fontsize=9,
    )
    style_axis(axis)


def draw_bar_section_header(axis: Any, metric: BarMetric) -> None:
    axis.set_axis_off()
    axis.text(
        0.0,
        0.85,
        f"{metric.section}. {metric.title}",
        color=TEXT,
        fontsize=14,
        fontweight="bold",
        ha="left",
        va="top",
        transform=axis.transAxes,
    )
    axis.text(
        0.0,
        0.48,
        f"Tiêu chí: {metric.criterion}",
        color=MUTED,
        fontsize=10,
        ha="left",
        va="top",
        transform=axis.transAxes,
    )
    axis.text(
        0.0,
        0.10,
        f"Thống kê: trung vị giữa các lượt chạy.  Đơn vị: {metric.axis_label}.  "
        f"{'Giá trị cao hơn tốt hơn.' if metric.higher_is_better else 'Giá trị thấp hơn tốt hơn.'}",
        color=MUTED,
        fontsize=9.5,
        ha="left",
        va="top",
        transform=axis.transAxes,
    )


def render_bar_summary(document: dict[str, Any], output: Path) -> None:
    figure = plt.figure(figsize=(13.5, 4.0 * len(BAR_METRICS) + 4), facecolor="white")
    figure.suptitle(
        "So sánh Veyra và mpv — tổng hợp benchmark (biểu đồ cột)",
        color=TEXT,
        fontsize=19,
        fontweight="bold",
        x=0.07,
        ha="left",
        y=0.98,
    )
    figure.text(0.07, 0.93, protocol_text(document), color=MUTED, fontsize=9.5, va="top")
    grid = figure.add_gridspec(
        len(BAR_METRICS) * 2,
        1,
        left=0.12,
        right=0.96,
        top=0.89,
        bottom=0.09,
        hspace=0.20,
        height_ratios=tuple(value for _ in BAR_METRICS for value in (0.34, 1.0)),
    )
    for index, metric in enumerate(BAR_METRICS):
        draw_bar_section_header(figure.add_subplot(grid[index * 2]), metric)
        draw_bar_metric(figure.add_subplot(grid[index * 2 + 1]), document, metric)
    figure.text(
        0.07,
        0.020,
        f"Nguồn log: {document.get('generated_at_utc', 'không rõ')}  |  "
        "Mỗi cột = trung vị; CPU có thể >100% khi dùng nhiều hơn một lõi.\n"
        "PSS phân bổ trang dùng chung; PSS shared phơi bày memfd/driver/texture. "
        "Media-clock FPS không phải số khung đã physically presented.",
        color=MUTED,
        fontsize=8.5,
    )
    save_png(figure, output)


def aggregate_time_series(
    document: dict[str, Any], player: str, metric: TimeMetric
) -> tuple[list[float], list[float], list[float], list[float]] | None:
    series_by_run: list[list[tuple[float, float]]] = []
    for run in document["runs"][player]:
        samples = run.get("metrics", {}).get(metric.sample_collection, [])
        series = [
            (float(sample["elapsed_seconds"]), metric.transform(float(sample[metric.sample_key])))
            for sample in samples
            if metric.sample_key in sample and "elapsed_seconds" in sample
        ]
        if series:
            series_by_run.append(series)
    if not series_by_run:
        return None

    time_values: list[float] = []
    median_values: list[float] = []
    minimum_values: list[float] = []
    maximum_values: list[float] = []
    for index in range(max(len(series) for series in series_by_run)):
        points = [series[index] for series in series_by_run if index < len(series)]
        times, values = zip(*points, strict=True)
        time_values.append(statistics.median(times))
        median_values.append(statistics.median(values))
        minimum_values.append(min(values))
        maximum_values.append(max(values))
    return time_values, median_values, minimum_values, maximum_values


def draw_time_metric(axis: Any, document: dict[str, Any], metric: TimeMetric) -> None:
    has_data = False
    for player in PLAYERS:
        aggregate = aggregate_time_series(document, player, metric)
        if aggregate is None:
            continue
        has_data = True
        times, medians, minimums, maximums = aggregate
        axis.fill_between(times, minimums, maximums, color=PLAYER_COLORS[player], alpha=0.14)
        axis.plot(
            times,
            medians,
            color=PLAYER_COLORS[player],
            linewidth=2.2,
            marker="o",
            markersize=3.5,
            label=PLAYER_LABELS[player],
        )
    style_axis(axis)
    axis.set_xlabel("Thời gian lấy mẫu đã trôi qua (giây)", color=TEXT, fontsize=10)
    axis.set_ylabel(metric.axis_label, color=TEXT, fontsize=10)
    axis.set_title(
        f"{metric.title}\n"
        f"Tiêu chí: {metric.criterion}\n"
        f"Đơn vị: {metric.axis_label}; đường = trung vị giữa các lượt; "
        "vùng mờ = nhỏ nhất–lớn nhất.",
        color=TEXT,
        fontsize=10.5,
        fontweight="bold",
        loc="left",
        pad=12,
        linespacing=1.4,
    )
    if not has_data:
        axis.text(
            0.5,
            0.5,
            "Log này chưa có mẫu theo khoảng thời gian.\n"
            "Hãy chạy lại benchmark.py để thu thập chuỗi thời gian.",
            transform=axis.transAxes,
            color=MUTED,
            fontsize=11,
            ha="center",
            va="center",
        )


def render_timeline(document: dict[str, Any], output: Path) -> None:
    figure, axes = plt.subplots(
        len(TIME_METRICS), 1, figsize=(13.5, 5.0 * len(TIME_METRICS)), facecolor="white"
    )
    figure.suptitle(
        "So sánh Veyra và mpv — diễn biến thông số khi phát (biểu đồ đường)",
        color=TEXT,
        fontsize=19,
        fontweight="bold",
        x=0.075,
        ha="left",
        y=0.985,
    )
    figure.text(
        0.075,
        0.94,
        protocol_text(document)
        + "\nĐộ trễ khởi động chỉ xảy ra một lần, nên chỉ được báo cáo trong biểu đồ cột.",
        color=MUTED,
        fontsize=9.5,
        va="top",
    )
    for axis, metric in zip(axes, TIME_METRICS, strict=True):
        draw_time_metric(axis, document, metric)
    legend_items = [
        Line2D([0], [0], color=VEYRA, linewidth=2.2, marker="o", markersize=4, label="Veyra trung vị"),
        Line2D([0], [0], color=MPV, linewidth=2.2, marker="o", markersize=4, label="mpv trung vị"),
        Line2D([0], [0], color=MUTED, linewidth=8, alpha=0.14, label="Vùng mờ: nhỏ nhất–lớn nhất"),
    ]
    figure.legend(
        handles=legend_items,
        loc="lower center",
        bbox_to_anchor=(0.5, 0.055),
        ncol=3,
        frameon=False,
        fontsize=9,
    )
    figure.text(
        0.075,
        0.018,
        f"Nguồn log: {document.get('generated_at_utc', 'không rõ')}  |  "
        "Mỗi mốc thời gian tổng hợp các vị trí lấy mẫu tương ứng từ những lượt chạy hoàn tất.",
        color=MUTED,
        fontsize=8.5,
    )
    figure.subplots_adjust(left=0.12, right=0.96, top=0.83, bottom=0.10, hspace=0.78)
    save_png(figure, output)


def save_png(figure: Any, output: Path) -> None:
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(f".{output.stem}.tmp.png")
    try:
        figure.savefig(temporary, format="png", dpi=180, facecolor="white")
        temporary.replace(output)
    finally:
        plt.close(figure)
        temporary.unlink(missing_ok=True)


def main() -> int:
    args = parse_args()
    if args.output.suffix.lower() != ".png":
        raise ValueError("--output must end in .png")
    args.timeline_output = args.timeline_output or args.output.with_name(
        f"{args.output.stem}-timeline.png"
    )
    if args.timeline_output.suffix.lower() != ".png":
        raise ValueError("--timeline-output must end in .png")
    document = json.loads(args.input.read_text(encoding="utf-8"))
    if not {"veyra", "mpv"}.issubset(document.get("runs", {})):
        raise ValueError("benchmark JSON is missing Veyra or mpv run data")
    render_bar_summary(document, args.output)
    render_timeline(document, args.timeline_output)
    print(f"Biểu đồ cột: {args.output}")
    print(f"Biểu đồ đường: {args.timeline_output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
