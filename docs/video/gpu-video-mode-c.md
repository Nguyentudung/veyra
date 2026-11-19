# Direct YUV handoff (Mode C)

Mode C is the experimental presentation path intended to remove Mode B's
intermediate BGRA render pass. Start it explicitly:

```sh
VEYRA_VIDEO_RENDERER=gpu-video ./target/release/veyra video_test/test.mp4
```

`software` remains the default. `vulkan`/`vulkan-readback` select the legacy
readback diagnostic renderer, and `gpu-native` selects Mode B.

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
VEYRA_VIDEO_RENDERER=software ./target/release/veyra video_test/test.mp4
VEYRA_VIDEO_RENDERER=gpu-native ./target/release/veyra video_test/test.mp4
VEYRA_VIDEO_RENDERER=gpu-video ./target/release/veyra video_test/test.mp4
pidstat -u -r -p "$(pgrep -n veyra)" 1 30
```

For Mode C confirm the renderer log says `renderer=gpu-video` and later reports
`[video] gpu-video:`. Test resize, seek, pause/resume and close, then compare
CPU, PSS, frame drops and starvation. Do not treat a successful build as a
performance result: runtime measurements are required.
