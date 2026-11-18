# libmpv rendering investigation

## Host and API

The development host has libmpv 0.41.0, `libmpv.so.2`, `libmpv-dev`, the `mpv` pkg-config module (2.5.0), and public headers under `/usr/include/mpv`. No system package installation is needed for a prototype here. The public client and render APIs are LGPL-2.1-or-later; mpv also contains GPL-licensed components, so the linked feature set and distribution obligations must be checked against the chosen build. Veyra is Apache-2.0; no mpv implementation code should be copied.

## Strategies inspected

| Strategy | Evidence and result | Assessment |
| --- | --- | --- |
| OpenGL render API | `render_gl.h` specifies a caller-owned current GL context, FBO rendering, thread affinity, and EGL plus the native X11/Wayland display for direct hardware decoding on Intel/Linux. Rune's `NativeSurface` accepts owned CPU byte frames and uploads them through its wgpu surface renderer. | Feasible as a separate EGL context + FBO + `glReadPixels` prototype, then row-stride/orientation conversion and `NativeSurface::push_bytes`. This adds a full-frame GPU-to-CPU readback and another upload, so it is a validation bridge, not a presumed production solution. Context creation, make-current, render, readback and destruction must remain on one render thread; resize recreates/reattaches the FBO; use pack alignment 1 and account for bottom-up GL rows. |
| Direct GPU composition | Rune's public app/UI API exposes byte submission. The inspected renderer owns internal wgpu `Device`, `Queue`, and texture resources, but no public external texture import/compositing hook was found. libmpv's OpenGL texture cannot be passed to wgpu without explicit interop and a compatible backend/device. | Not currently available through Rune's public API. A Rune API/fork or supported EGLImage/DMA-BUF interop is needed to avoid readback. |
| Native child video window | Normal mpv output can target a native window handle, but Rune does not expose a public child-surface attachment API. A separate top-level window would cover Rune controls and playlist and complicate fullscreen, resize and input. | Not suitable for product composition; useful only as a standalone mpv reference benchmark. |
| Rust bindings | No maintained libmpv binding was found in the current project dependency set/cache. The installed headers allow bindgen or a maintained crate with matching ABI. | Prefer a maintained binding after the render route and ownership model are proven. |

## Prototype and decision

No real-video Rune/libmpv rendering prototype or performance benchmark has been run. The render API route is implementable, but readback may cost substantially at high resolutions: a 3840×2160 RGBA frame is about 31.6 MiB, or about 1.85 GiB/s at 60 fps before Rune uploads it. This is a bandwidth estimate, not a measurement. The prototype must measure CPU/GPU time, dropped frames, memory and resize/cleanup behavior on the target desktop.

Recommended experiment: isolated Linux EGL pbuffer/context, libmpv OpenGL render API into an FBO, `glReadPixels` into a reusable buffer, then `NativeSurface::push_bytes`; test X11 and Wayland display integration before enabling hardware decoding. If readback cost is unacceptable, pause migration until Rune exposes texture interop or a supported native composition design. Keep the current engine until the prototype demonstrates stable video and overlay presentation.

The architecture decision is provisional: libmpv remains the preferred playback core, but migration is not yet supported by prototype evidence. This document records inspected APIs, not tested rendering.
