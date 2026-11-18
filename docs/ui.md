# User interface

## Status

**Implemented**. Veyra uses a video-first Rune window with native decorations, a responsive letterboxed BGRA surface, a top-center filename capsule, top-corner fullscreen and playlist capsules, a bottom-centered playback capsule, a separate bottom timeline, and a bottom-right hover-revealed vertical volume control. Shared glass styling lives in `src/ui/styles.rs`.

## Module responsibilities

- `src/ui/mod.rs`: Rune application state, video-first overlay composition, Rune Kit integration, callbacks, playback commands, playlist synchronization, and frame handoff.
- `src/ui/lucide.rs`: embedded Lucide geometry used consistently by interactive controls.
- `src/video/rendering/renderer.rs`: software conversion into BGRA bytes accepted by Rune's native surface.

Rune's poll hook checks the engine on the UI event loop, takes the newest `VideoFrame`, and moves its BGRA allocation directly into `NativeSurface::push_bytes`. No PNG/JPEG encoding, decoder, ALSA pointer, or worker-owned Rune object crosses this boundary.

The fullscreen action uses Rune's `Tx::toggle_full_screen` and lives in its own top-left capsule. The playlist toggle lives in a matching top-right capsule and opens a floating panel without reducing the video viewport. Pause/mute state is derived from the shared snapshot, while volume is held by the UI state and sent to the engine. The bottom-right volume capsule reveals a compact vertical slider above the mute button on hover, with a slim track and round thumb. Volume changes only during an intentional click/drag gesture on the slider; pointer movement over the mute button only reveals the slider. Previous/next and relative seek actions remain in the bottom-centered playback capsule. The timeline is independently anchored above the playback capsule and supports intentional click/drag seeking when duration is available.

The application uses Rune's native window backend and standard decorations. This preserves window-manager dragging, minimizing, maximizing/restoring, resizing, fullscreen, and closing behavior on Linux while keeping the UI free of a custom titlebar.

## Known UI limitations

The `zenity` subprocess is synchronous and Linux-specific. Frame presentation moves the latest BGRA allocation into Rune's bounded native-surface slot; Rune consumes it on draw. Drag/drop remains future work. General playback controls remain visible to avoid flicker; the volume slider uses Rune hover and drag events because its interaction boundary is local and reliable.
