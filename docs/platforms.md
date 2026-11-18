# Platform and dependency support

## Linux

**Status: Needs Verification.** Linux is the only configured target. The build links system `avformat`, `avcodec`, `avutil`, `swscale`, `swresample`, and `asound`. Runtime file selection uses `zenity`; build-time FFI generation uses `clang`/libclang. Ubuntu GUI, ALSA device availability, and representative media playback need fixture-based verification.

## Windows and macOS

**Status: Planned.** No platform backend, FFmpeg packaging strategy, or native audio-output implementation is configured for Windows or macOS. Do not claim support.

## Dependencies and licensing

Veyra is Apache-2.0. Direct Rust dependencies are Rune, Rune Kit, and libc 0.2.191; the build-time dependency is bindgen. Rune and Rune Kit are pinned to the `cca5df6fafb4721d96fe105f126ebfd2ec351bb6` development revision. Linux builds require FFmpeg development libraries, ALSA development headers, Clang/libclang for bindgen, and `zenity` for the file chooser. Rune's MIT license and bundled third-party notices must be preserved for redistribution.

FFmpeg's effective license depends on the installed build configuration; some configurations are GPL and codecs can carry patent obligations. ALSA is LGPL-2.1-or-later. Veyra links system shared libraries and ships no codec binaries. Before distribution, record exact FFmpeg configure flags, notices, transitive Rust licenses, and codec/patent review.
