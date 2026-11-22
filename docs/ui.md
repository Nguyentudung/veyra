# User interface

## Status

**Implemented**. Veyra uses a video-first Rune window with native decorations, a responsive letterboxed native surface, a top-center filename pill, top-corner fullscreen and info controls, a bottom-centered playback capsule, a separate bottom timeline, and a bottom-right hover-revealed vertical volume control. Shared glass and opaque surface styling lives in `src/ui/src/theme.rs`.

## Module responsibilities

- `src/ui/src/lib.rs`: Rune application state, main layout assembly, overlay visibility, callbacks, playback commands, playlist synchronization, and frame handoff.
- `src/ui/src/theme.rs`: shared color, glass, opaque surface, border, and hover tokens.
- `src/ui/src/titlebar.rs`: native Rune window-chrome integration seam.
- `src/ui/src/top_bar.rs`: fullscreen, filename pill, and Statistics info control.
- `src/ui/src/playback_controls.rs`: transport, timeline, timestamps, and volume controls.
- `src/ui/src/options_menu.rs`: bottom-left tools menu and popup actions.
- `src/ui/src/statistics_panel.rs`: tabbed Statistics / Media Info popup.
- `src/ui/src/lucide.rs`: embedded Lucide geometry used consistently by interactive controls.
- `src/video/src/rendering/gpu_video.rs`: direct Y/UV uploads to Rune's native compositor; `renderer.rs` retains the software BGRA fallback.

Rune's poll hook takes the newest `VideoFrame` on the UI event loop. GPU YUV frames go directly to `NativeSurface::push_gpu_video`; unsupported formats fall back to `push_bytes`. No PNG/JPEG encoding, decoder, ALSA pointer, or worker-owned Rune object crosses this boundary.

The fullscreen action uses Rune's `Tx::toggle_full_screen` and lives in its own top-left capsule. The info control anchors the opaque Statistics panel beneath the top-right button. Pause/mute state is derived from the shared snapshot, while volume is held by the UI state and sent to the engine. The bottom-right volume capsule reveals a compact vertical slider above the mute button on hover, with a slim track and round thumb. Previous/next and relative seek actions remain in the bottom-centered playback capsule. The timeline is independently anchored above the playback capsule and supports intentional click/drag seeking when duration is available.

The application uses Rune's native window backend and standard decorations. This preserves window-manager dragging, minimizing, maximizing/restoring, resizing, fullscreen, and closing behavior on Linux while keeping the UI free of a custom titlebar.

## Known UI limitations

The `zenity` subprocess is synchronous and Linux-specific. Frame presentation uses a bounded latest-wins native-surface slot; Rune consumes it on draw. Drag/drop remains future work. General playback controls remain visible to avoid flicker; the volume slider uses Rune hover and drag events because its interaction boundary is local and reliable.
