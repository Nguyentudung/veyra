# GPU-native video presentation (Mode B)

Mode B removes the old Vulkan readback loop while retaining the FFmpeg software
decoder and Rune's normal compositor. It is deliberately an opt-in experiment:

```sh
VEYRA_VIDEO_RENDERER=gpu-native ./target/release/veyra video_test/test.mp4
```

`software` remains the default. `vulkan` and `vulkan-readback` select the
legacy diagnostic path, which owns a second Vulkan device and reads BGRA back
to the CPU; they are not the native route.

## Data path

```text
FFmpeg AVFrame (YUV420P)
  -> queue.write_texture(Y, U, V) on Rune's Queue
  -> Veyra YUV-to-BGRA render pass on Rune's Device
  -> leased BGRA TextureView in the latest-wins mailbox
  -> NativeSurface::push_gpu
  -> Rune samples that view in its normal composite pass
```

There is no `map_async`, `Device::poll`, BGRA readback buffer, CPU BGRA `Vec`,
or subsequent `NativeSurface::push_bytes` upload on this route. The first UI
frame publishes Rune's `GpuContext` to the media worker. If a video is opened
before that happens, the worker uses the existing software renderer until the
context arrives; it does not queue decoded frames.

## Lifetime and back-pressure

`src/video/src/rendering/gpu_native.rs` owns four BGRA output textures. A texture
slot is leased into `rune::native_surface::GpuFrame` when it is published.
Rune keeps its imported frame in the normal prepared draw and moves it into its
submission-completion callback. Dropping that last frame drops Veyra's lease,
which returns the slot to the pool without issuing any wgpu call.

The four slots cover the producer, the Veyra mailbox, Rune's prepared frame,
and a submission still sampled by the GPU. If all slots are leased, Veyra drops
the newly decoded frame instead of waiting. It reports the `starved` counter in
the lightweight `[video] gpu-native` diagnostic every 300 submitted frames.
This is bounded presentation back-pressure, not an unbounded frame queue.

Pool replacement occurs only when the decoded picture dimensions change. Old
pools remain alive only through their outstanding frame leases and are released
after Rune has stopped using the associated views.

## Integration constraint

Both sides must use exactly Rune's `wgpu` types. Veyra's native module imports
`rune::wgpu`; it does not pass resources to the separate `wgpu` dependency
used solely by the legacy readback renderer. Rune, including the
`GpuContext`, `GpuFrame`, `NativeSurface::gpu_context`, and
`NativeSurface::push_gpu` bridge, is vendored under `third_party/rune`.

## Validation procedure

1. Build `cargo build --release`.
2. Start the command above and confirm both messages appear once:
   `renderer=gpu-native` and `gpu-native renderer attached to Rune device`.
   Their absence means the software fallback is still presenting frames.
3. Watch the video through a resize, seek, pause/resume, and close. The normal
   software path (`VEYRA_VIDEO_RENDERER=software`) must still work unchanged.
4. For each renderer, use the same input and display size:

   ```sh
   pidstat -u -r -p "$(pgrep -n veyra)" 1 30
   awk '/Pss:|Private_/ {sum += $2} END {print sum " kB"}' \
     /proc/"$(pgrep -n veyra)"/smaps_rollup
   ```

   RSS includes shared libraries and Vulkan driver mappings; PSS is the better
   cross-player memory comparison. Record CPU, PSS, FPS, frame drops and the
   `starved` count. Do not claim an improvement until this comparison is made.

The existing 30-second readback log is useful only as a baseline: it warmed
from 167.4 MB to about 181.3 MB RSS then plateaued, while CPU averaged 35.15%.
It is evidence against an immediate unbounded RSS leak in that interval, not a
measurement of Mode B.
