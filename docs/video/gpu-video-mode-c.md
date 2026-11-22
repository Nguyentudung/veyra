# Direct YUV handoff (Mode C)

Mode C is Veyra's default presentation path. It removes Mode B's intermediate
BGRA render pass and is selected automatically for 8-bit YUV420P/NV12 video.
For reproducible non-interactive profiling, set `VEYRA_OPEN_MEDIA`:

```sh
VEYRA_OPEN_MEDIA=video_test/test.mp4 ./target/release/veyra
```

`software` remains the explicit portable fallback. `vulkan`/`vulkan-readback`
select the legacy readback diagnostic renderer, and `gpu-native` selects Mode B.

## Path and supported input

```text
FFmpeg AVFrame (NV12 or YUV420P)
  -> queue.write_texture(Y + RG UV) on Rune's Queue
  -> leased GPU-video mailbox frame
  -> Rune's existing NV12 compositor (YUV conversion + scale + present)
```

There is no Veyra command encoder, render pass, BGRA output texture, GPU
readback, map/poll wait, or Veyra `queue.submit` on this path. NV12 uploads
both planes directly. YUV420P interleaves U and V into one reusable CPU buffer
for Rune's RG8 UV input; that buffer retains capacity and is not allocated per
frame.

The implementation currently accepts 8-bit `YUV420P` and `NV12`. Other pixel
formats or unsupported color metadata fall back to the software renderer rather
than silently applying an incorrect HDR/BT.2020 conversion. Limited/full
range and BT.601/BT.709 metadata are passed to Rune's existing video shader.

## Ownership and pressure

Veyra owns a four-slot Y/UV texture pool. A frame carries an ownership-only
lease; Rune retains it through the compositor submission completion callback.
When all slots are in use, Veyra drops the newest decoded frame and increments
the `starved` diagnostic instead of waiting or growing a queue. Slot release
does not issue wgpu calls, so it is safe from the completion callback thread.

## Validation

Build and run the same clip at the same window size for each mode:

```sh
cargo build --release
VEYRA_OPEN_MEDIA=video_test/test.mp4 VEYRA_VIDEO_RENDERER=software ./target/release/veyra
VEYRA_OPEN_MEDIA=video_test/test.mp4 VEYRA_VIDEO_RENDERER=gpu-native ./target/release/veyra
VEYRA_OPEN_MEDIA=video_test/test.mp4 VEYRA_VIDEO_RENDERER=gpu-video ./target/release/veyra
pidstat -u -r -p "$(pgrep -n veyra)" 1 30
```

For Mode C confirm the renderer log says `renderer=gpu-video` and later reports
`[video] gpu-video:`. Test resize, seek, pause/resume and close, then compare
CPU, PSS, frame drops and starvation. Do not treat a successful build as a
performance result: runtime measurements are required.

## Initial audit (not a benchmark)

The Mode C producer allocates its Y/UV textures only when decoded dimensions
change and reuses a four-slot pool. It calls `Queue::write_texture` for Y and
UV, then publishes the frame; it does not create a command encoder, render
pass, submission, `map_async`, or `Device::poll`. Rune imports the two views
without uploading them again and retains the frame until its compositor
submission completes.

There are still small per-frame ownership allocations: the published frame and
its RAII lease are each reference-counted heap objects. They are not a texture
or pixel-buffer allocation, and no profile currently attributes a meaningful
cost to them. The YUV420P UV interleave buffer is retained and reused.

ALSA is intentionally reset on every seek to prevent pre-seek PCM playing
against the new video clock. The log now identifies this explicitly as
`[audio] resetting output for seek ...`; an `ALSA default opened #N` message
without a preceding seek/reset indicates an open-media or lifecycle event that
should be investigated. The AAC timestamp and `env_facs_q` messages are
emitted by FFmpeg's AAC decoder, not by Vulkan or Mode C.

## Functional evidence

| Case | Status | Evidence / limit |
| --- | --- | --- |
| Renderer attachment | Passed | Runtime log reported `gpu-video renderer attached to Rune device`. |
| Continuous presentation | Passed | 900 uploads completed with `starved=0`, pool id unchanged and two free slots. |
| CPU upload reuse | Passed | YUV420P upload was 0.22–0.27 ms and UV interleave 0.07–0.09 ms; these are point samples, not averages. |
| No producer readback/pass/submit | Passed by source audit | `NativeVideoRenderer::upload` only writes the two planes and publishes the frame. |
| Lease lifetime | Passed by source audit | Rune retains `Imported` through its compositor completion callback before dropping the producer lease. |
| Pause/resume, forward/back seek, EOS/restart | Not yet run as a controlled test | Must be tested interactively after the release build. |
| Resolution/pixel-format transition | Not yet run as a controlled test | A size transition makes a new pool; unsupported formats intentionally fall back to software. |
| Pixel-accurate color comparison | Not yet run | BT.601/709 and limited/full metadata are wired, but a reference-image test is still needed. |

## Benchmark record

Earlier exploratory captures suggested about 30% CPU / 174 MB RSS for software
and roughly 35% CPU / 180 MB RSS for the old readback and Mode B paths. They
were not collected under one controlled protocol and must not be used as a
renderer comparison. Mode C has **no CPU/RSS result yet**; the confirmed Mode C
numbers below describe only producer work.

| Renderer | Active-playback CPU | RSS/PSS | Video-path observation | Comparison validity |
| --- | ---: | ---: | --- | --- |
| Software | Not remeasured | Not remeasured | CPU conversion/upload path | Pending controlled run |
| Vulkan readback | Not remeasured | Not remeasured | Has encoder, BGRA readback and polling | Pending controlled run |
| Mode B | Not remeasured | Not remeasured | Has Veyra YUV-to-BGRA pass and an extra submit | Pending controlled run |
| Mode C | Not measured in this document | 184 MB RSS / 122 MB PSS at 8 s on 1080×1920 H.264/AAC, 60 fps | 1.29 ms upload; 0.92 ms YUV420P interleave; no starvation after 300 uploads | One-host point sample; compare under the same protocol |

For each row, play the same file at the same window size for 60–120 seconds,
wait for warm-up, and record only active playback. Capture `pidstat` and a
Mode-C profile during the same interval:

```sh
PID="$(pgrep -n veyra)"
pidstat -u -r -p "$PID" 1 60
perf record -e cpu-clock -g -p "$PID" -- sleep 30
perf report --stdio --no-children --sort comm,dso | head -40
```

Use `cpu-clock` to avoid the hybrid-CPU PMU event error seen with default
`perf record`. If `perf_event_paranoid` blocks recording, adjust it only with
the system administrator's approval.
