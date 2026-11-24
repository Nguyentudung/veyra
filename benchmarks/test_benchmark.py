"""Unit tests for the deterministic parts of the benchmark harness."""

from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest

from benchmark import (
    BenchmarkError,
    append_final_playback_sample,
    annotate_mpv_playback_samples,
    annotate_veyra_playback_samples,
    parse_mpv_ipc_responses,
    read_veyra_playback_metrics,
    summarize_mpv_playback,
    summarize_playback_metrics,
    summarize_veyra_playback,
)


class PlaybackMetricsTests(unittest.TestCase):
    def test_reads_player_telemetry(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "metrics.json"
            path.write_text(
                json.dumps(
                    {
                        "schema_version": 2,
                        "video_frames_processed": 120,
                        "rendered_frames": 118,
                        "playback_position_seconds": 2.0,
                        "render_fps": 59.4,
                        "dropped_frames": 2,
                        "decode_threads": 4,
                    }
                ),
                encoding="utf-8",
            )
            self.assertEqual(
                read_veyra_playback_metrics(path),
                {
                    "video_frames_processed": 120,
                    "rendered_frames": 118,
                    "playback_position_seconds": 2.0,
                    "render_fps": 59.4,
                    "dropped_frames": 2,
                    "decode_threads": 4,
                },
            )

    def test_rejects_incomplete_player_telemetry(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "metrics.json"
            path.write_text(
                json.dumps({"schema_version": 1, "rendered_frames": 42}),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(BenchmarkError, "missing"):
                read_veyra_playback_metrics(path)

    def test_summarizes_only_available_playback_fields(self) -> None:
        summary = summarize_playback_metrics(
            [
                {
                    "playback_metrics": {
                        "video_frames_processed": 60,
                        "rendered_frames": 59,
                        "render_fps": 58.5,
                        "dropped_frames": 1,
                    }
                },
                {
                    "playback_metrics": {
                        "video_frames_processed": 90,
                        "rendered_frames": 89,
                        "render_fps": 60.0,
                        "dropped_frames": 1,
                    }
                },
            ]
        )
        self.assertEqual(summary["render_fps"], {"median": 59.25, "max": 60.0})
        self.assertEqual(summary["dropped_frames"], {"median": 1.0, "max": 1.0})
        self.assertNotIn("frame_jitter_ms", summary)

    def test_veyra_fps_uses_counter_delta_over_the_measurement_window(self) -> None:
        samples = [
            {
                "elapsed_seconds": 0.0,
                "rendered_frames": 100,
                "video_frames_processed": 101,
                "playback_position_seconds": 4.0,
                "dropped_frames": 1,
                "render_fps": 58.0,
                "decode_threads": 4,
            },
            {
                "elapsed_seconds": 2.0,
                "rendered_frames": 218,
                "video_frames_processed": 222,
                "playback_position_seconds": 6.0,
                "dropped_frames": 4,
                "render_fps": 60.0,
                "decode_threads": 4,
            },
        ]
        annotate_veyra_playback_samples(samples, source_video_fps=60.0)
        summary = summarize_veyra_playback(samples, source_video_fps=60.0)
        self.assertEqual(samples[1]["delivery_fps"], 59.0)
        self.assertEqual(samples[1]["media_clock_fps_estimate"], 60.0)
        self.assertEqual(summary["delivery_fps"], 59.0)
        self.assertEqual(summary["media_clock_fps_estimate"], 60.0)
        self.assertEqual(summary["media_clock_speed"], 1.0)
        self.assertEqual(summary["processed_fps"], 60.5)
        self.assertEqual(summary["frame_drop_count"], 3)
        self.assertEqual(summary["frame_drop_fps"], 1.5)
        self.assertEqual(summary["render_fps_rolling"], 60.0)

    def test_final_playback_snapshot_is_appended_and_excluded_from_timeline(self) -> None:
        metrics = {
            "duration_seconds": 5.0,
            "playback_samples": [
                {"elapsed_seconds": 0.0, "rendered_frames": 100},
                {"elapsed_seconds": 4.996, "rendered_frames": 390},
            ],
        }

        append_final_playback_sample(metrics, {"rendered_frames": 391})

        self.assertEqual(len(metrics["playback_samples"]), 3)
        self.assertEqual(
            metrics["playback_samples"][-2],
            {"elapsed_seconds": 4.996, "rendered_frames": 390},
        )
        self.assertEqual(
            metrics["playback_samples"][-1],
            {
                "elapsed_seconds": 5.0,
                "rendered_frames": 391,
                "snapshot_only": True,
            },
        )

    def test_final_snapshot_keeps_a_short_window_measurable(self) -> None:
        samples = [
            {
                "elapsed_seconds": 0.0,
                "rendered_frames": 100,
                "video_frames_processed": 100,
                "dropped_frames": 0,
            }
        ]
        metrics = {"duration_seconds": 0.1, "playback_samples": samples}
        append_final_playback_sample(
            metrics,
            {
                "rendered_frames": 106,
                "video_frames_processed": 106,
                "dropped_frames": 0,
            },
        )

        summary = summarize_veyra_playback(samples)

        self.assertEqual(summary["delivery_fps"], 60.0)
        self.assertEqual(summary["processed_fps"], 60.0)

    def test_final_snapshot_does_not_add_a_short_interval_fps_point(self) -> None:
        samples = [
            {
                "elapsed_seconds": 0.0,
                "rendered_frames": 100,
                "video_frames_processed": 100,
                "playback_position_seconds": 1.0,
                "dropped_frames": 0,
            },
            {
                "elapsed_seconds": 4.996,
                "rendered_frames": 390,
                "video_frames_processed": 400,
                "playback_position_seconds": 5.8,
                "dropped_frames": 10,
            },
        ]
        metrics = {"duration_seconds": 5.0, "playback_samples": samples}
        append_final_playback_sample(
            metrics,
            {
                "rendered_frames": 391,
                "video_frames_processed": 401,
                "playback_position_seconds": 5.816667,
                "dropped_frames": 10,
            },
        )

        annotate_veyra_playback_samples(samples, source_video_fps=60.0)

        self.assertNotIn("delivery_fps", samples[-1])
        self.assertNotIn("media_clock_fps_estimate", samples[-1])

    def test_mpv_keeps_scheduled_cadence_separate_from_cfr_media_clock(self) -> None:
        samples = [
            {
                "elapsed_seconds": 0.0,
                "display_sync_active": True,
                "estimated_display_fps": 60.0,
                "vsync_ratio": 0.5,
                "playback_time_seconds": 4.0,
                "vo_frame_drop_count": 2,
                "decoder_frame_drop_count": 1,
                "mistimed_frame_count": 3,
                "vo_delayed_frame_count": 4,
            },
            {
                "elapsed_seconds": 2.0,
                "display_sync_active": True,
                "estimated_display_fps": 60.0,
                "vsync_ratio": 0.5,
                "playback_time_seconds": 6.0,
                "vo_frame_drop_count": 5,
                "decoder_frame_drop_count": 1,
                "mistimed_frame_count": 5,
                "vo_delayed_frame_count": 6,
            },
        ]
        annotate_mpv_playback_samples(samples, source_video_fps=60.0)
        summary = summarize_mpv_playback(samples, source_video_fps=60.0)
        self.assertNotIn("playback_fps", samples[0])
        self.assertEqual(samples[0]["display_sync_scheduled_cadence_fps"], 120.0)
        self.assertEqual(samples[1]["media_clock_fps_estimate"], 60.0)
        self.assertEqual(summary["media_clock_fps_estimate"], 60.0)
        self.assertEqual(summary["source_frame_admission_fps_estimate"], 58.5)
        self.assertEqual(summary["display_sync_scheduled_cadence_fps"], 120.0)
        self.assertEqual(summary["vo_frame_drop_count"], 3)
        self.assertEqual(summary["decoder_frame_drop_count"], 0)
        self.assertEqual(summary["mistimed_frame_count"], 2)

    def test_mpv_media_clock_fps_survives_missing_display_sync(self) -> None:
        samples = [
            {
                "elapsed_seconds": 0.0,
                "display_sync_active": False,
                "playback_time_seconds": 10.0,
                "vo_frame_drop_count": 0,
                "decoder_frame_drop_count": 0,
            },
            {
                "elapsed_seconds": 2.0,
                "display_sync_active": False,
                "playback_time_seconds": 12.0,
                "vo_frame_drop_count": 0,
                "decoder_frame_drop_count": 0,
            },
        ]
        annotate_mpv_playback_samples(samples, source_video_fps=60.0)
        summary = summarize_mpv_playback(samples, source_video_fps=60.0)
        self.assertEqual(summary["media_clock_fps_estimate"], 60.0)
        self.assertEqual(summary["source_frame_admission_fps_estimate"], 60.0)
        self.assertNotIn("display_sync_scheduled_cadence_fps", summary)
        self.assertEqual(summary["display_sync_unavailable_reason"], "no_display_sync")

    def test_unknown_or_non_monotonic_clock_never_makes_up_fps(self) -> None:
        samples = [
            {
                "elapsed_seconds": 0.0,
                "playback_time_seconds": 10.0,
                "vo_frame_drop_count": 0,
                "decoder_frame_drop_count": 0,
            },
            {
                "elapsed_seconds": 2.0,
                "playback_time_seconds": 9.0,
                "vo_frame_drop_count": 0,
                "decoder_frame_drop_count": 0,
            },
        ]
        annotate_mpv_playback_samples(samples, source_video_fps=None)
        summary = summarize_mpv_playback(samples, source_video_fps=None)
        self.assertNotIn("media_clock_fps_estimate", summary)

    def test_mpv_ipc_ignores_events_and_matches_request_ids(self) -> None:
        payload = b"\n".join(
            (
                b'{"event":"video-reconfig"}',
                b'{"data":7,"request_id":8100,"error":"success"}',
                b'{"request_id":8101,"error":"property not found"}',
            )
        )
        self.assertEqual(
            parse_mpv_ipc_responses(
                payload,
                {8100: "frame-drop-count", 8101: "vo-presentation-delay"},
            ),
            {"frame-drop-count": 7, "vo-presentation-delay": None},
        )


if __name__ == "__main__":
    unittest.main()
