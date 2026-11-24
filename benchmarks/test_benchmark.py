"""Unit tests for the deterministic parts of the benchmark harness."""

from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest

from benchmark import (
    BenchmarkError,
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
                        "schema_version": 1,
                        "video_frames_processed": 120,
                        "rendered_frames": 118,
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
                "dropped_frames": 1,
                "render_fps": 58.0,
                "decode_threads": 4,
            },
            {
                "elapsed_seconds": 2.0,
                "rendered_frames": 218,
                "video_frames_processed": 222,
                "dropped_frames": 4,
                "render_fps": 60.0,
                "decode_threads": 4,
            },
        ]
        annotate_veyra_playback_samples(samples)
        summary = summarize_veyra_playback(samples)
        self.assertEqual(samples[1]["playback_fps"], 59.0)
        self.assertEqual(summary["playback_fps"], 59.0)
        self.assertEqual(summary["processed_fps"], 60.5)
        self.assertEqual(summary["frame_drop_count"], 3)
        self.assertEqual(summary["frame_drop_fps"], 1.5)
        self.assertEqual(summary["render_fps_rolling"], 60.0)

    def test_mpv_fps_is_only_reported_from_display_sync(self) -> None:
        samples = [
            {
                "elapsed_seconds": 0.0,
                "display_sync_active": True,
                "estimated_display_fps": 60.0,
                "vsync_ratio": 0.5,
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
                "vo_frame_drop_count": 5,
                "decoder_frame_drop_count": 1,
                "mistimed_frame_count": 5,
                "vo_delayed_frame_count": 6,
            },
        ]
        annotate_mpv_playback_samples(samples)
        summary = summarize_mpv_playback(samples)
        self.assertEqual(samples[0]["playback_fps"], 60.0)
        self.assertEqual(samples[0]["display_sync_scheduled_fps"], 120.0)
        self.assertEqual(summary["playback_fps"], 60.0)
        self.assertEqual(summary["display_sync_scheduled_fps"], 120.0)
        self.assertEqual(summary["vo_frame_drop_count"], 3)
        self.assertEqual(summary["decoder_frame_drop_count"], 0)
        self.assertEqual(summary["mistimed_frame_count"], 2)

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
