# User interface

## Status

**Implemented.** Veyra now uses a video-first GPUI window with a responsive letterboxed video canvas, a top-center filename pill, top-corner fullscreen and Statistics controls, a bottom-centered playback capsule, a separate bottom timeline, and a bottom-right hover-revealed vertical volume control.

The welcome view provides an open-file action and keyboard hint. Audio-only
files use an artwork placeholder and the same playback controls as video.
The top controls also expose the playlist and an open-file action. The
playlist highlights the current item and can switch directly to another.
Keyboard shortcuts are listed in the tools menu; Escape closes the overlays
or exits fullscreen.

## Module responsibilities

- `src/ui/src/lib.rs`: the complete GPUI application state and view tree: overlays, input callbacks, playback commands, playlist synchronization, and image handoff.
- `src/video/src/rendering/renderer.rs`: converts decoded frames to portable BGRA bytes for the GPUI frontend.
- `src/video/src/mailbox.rs`: keeps only the most recent frame, so the UI cannot build up a backlog.

The GPUI poll task takes the newest `VideoFrame` on the UI event loop and creates a `RenderImage` from its BGRA bytes. No PNG/JPEG encoding, decoder, ALSA pointer, or worker-owned media object crosses this boundary. Replacing the stored image drops the prior GPUI image, so the presentation side also remains latest-wins.

The fullscreen action uses GPUI's native window action and lives in its own top-left capsule. The info control anchors the opaque Statistics panel beneath the top-right button. Pause/mute state is derived from the shared snapshot, while volume is held by the UI state and sent to the engine. The bottom-right volume capsule reveals a compact vertical slider above the mute button on hover. Previous/next and relative seek actions remain in the bottom-centered playback capsule. The timeline is independently anchored above the playback capsule and supports click/drag seeking when duration is available.

The application requests client-side decorations and draws its own draggable titlebar with minimize, maximize/restore and close controls, plus resize handles around the window. These controls are hidden in fullscreen.

## Known UI limitations

The `zenity` subprocess is synchronous and Linux-specific. Frame presentation uses a bounded latest-wins mailbox, then uploads a BGRA image to GPUI. The default video conversion is software; `vulkan-readback` remains available as an experimental alternative. Drag/drop remains future work.
