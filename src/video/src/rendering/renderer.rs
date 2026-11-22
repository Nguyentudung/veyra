use super::super::ffi;
use super::frame::VideoFrame;
use super::gpu::GpuRenderer;
use super::gpu_native::NativeGpuRenderer;
use super::gpu_video::NativeVideoRenderer;
use crate::GpuContext;

// libswscale can use vector stores past the final visible BGRA pixel. Keep its
// rows 64-byte aligned, then pack them before the existing tight ByteFrame API.
const SWS_OUTPUT_ROW_ALIGNMENT: usize = 64;

pub(crate) struct SoftwareRenderer {
    context: *mut ffi::SwsContext,
    /// A diagnostics-only canary after the BGRA image. This is never enabled
    /// during normal playback and lets us prove an out-of-bounds destination
    /// write before the allocator reports later heap corruption.
    output_guard_bytes: usize,
}

impl SoftwareRenderer {
    pub(crate) fn new() -> Self {
        Self {
            context: std::ptr::null_mut(),
            output_guard_bytes: matches!(
                std::env::var("VEYRA_VIDEO_DIAGNOSTICS").as_deref(),
                Ok("scale-guard")
            )
            .then_some(64 * 1024)
            .unwrap_or(0),
        }
    }
    pub(crate) unsafe fn convert(
        &mut self,
        frame: *mut ffi::AVFrame,
    ) -> Result<(Vec<u8>, u32, u32), String> {
        let width = (*frame).width;
        let height = (*frame).height;
        if self.context.is_null() {
            self.context = ffi::sws_getContext(
                width,
                height,
                (*frame).format,
                width,
                height,
                ffi::AVPixelFormat_AV_PIX_FMT_BGRA,
                2,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
        }
        if self.context.is_null() {
            return Err("could not create software pixel converter".into());
        }
        let width = usize::try_from(width).map_err(|_| "invalid software frame width")?;
        let height = usize::try_from(height).map_err(|_| "invalid software frame height")?;
        let tight_stride = width.checked_mul(4).ok_or("software frame size overflow")?;
        let output_stride = tight_stride
            .checked_add(SWS_OUTPUT_ROW_ALIGNMENT - 1)
            .ok_or("software frame stride overflow")?
            / SWS_OUTPUT_ROW_ALIGNMENT
            * SWS_OUTPUT_ROW_ALIGNMENT;
        let image_bytes = tight_stride
            .checked_mul(height)
            .ok_or("software frame size overflow")?;
        let scaled_bytes = output_stride
            .checked_mul(height)
            .ok_or("software frame buffer size overflow")?;
        let mut rgba = vec![0u8; scaled_bytes];
        if self.output_guard_bytes != 0 {
            // Use a non-zero pattern: a zero-filled guard could fail to detect
            // an overrun made solely of zero-valued color components.
            rgba.resize(scaled_bytes + self.output_guard_bytes, 0xA5);
        }
        let mut data = [
            rgba.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ];
        let lines = [
            i32::try_from(output_stride).map_err(|_| "software frame stride too large")?,
            0,
            0,
            0,
        ];
        let rows_written = ffi::sws_scale(
            self.context,
            (*frame).data.as_ptr() as *const *const u8,
            (*frame).linesize.as_ptr(),
            0,
            i32::try_from(height).map_err(|_| "software frame height too large")?,
            data.as_mut_ptr(),
            lines.as_ptr(),
        );
        if rows_written != height as i32 {
            return Err(format!(
                "sws_scale produced {rows_written} rows for a {height}-row frame"
            ));
        }
        let guard = &rgba[scaled_bytes..];
        if self.output_guard_bytes != 0
            && let Some(first_changed) = guard.iter().position(|byte| *byte != 0xA5)
        {
            let last_changed = guard
                .iter()
                .rposition(|byte| *byte != 0xA5)
                .unwrap_or(first_changed);
            let changed_bytes = guard.iter().filter(|byte| **byte != 0xA5).count();
            return Err(format!(
                "sws_scale wrote beyond the {scaled_bytes}-byte BGRA destination (guard changed: {changed_bytes} bytes, first: {first_changed}, last: {last_changed}, guard: {} bytes)",
                self.output_guard_bytes,
            ));
        }
        // Rune's established ByteFrame path takes tightly packed rows. Copy
        // away only the alignment padding that libswscale needs internally.
        if output_stride == tight_stride {
            rgba.truncate(image_bytes);
            return Ok((rgba, width as u32, height as u32));
        }
        let mut packed = vec![0u8; image_bytes];
        for row in 0..height {
            let source_start = row * output_stride;
            let target_start = row * tight_stride;
            packed[target_start..target_start + tight_stride]
                .copy_from_slice(&rgba[source_start..source_start + tight_stride]);
        }
        Ok((packed, width as u32, height as u32))
    }
}

impl Drop for SoftwareRenderer {
    fn drop(&mut self) {
        unsafe {
            if !self.context.is_null() {
                ffi::sws_freeContext(self.context);
            }
        }
    }
}

pub(crate) enum VideoRenderer {
    /// Temporary ownership placeholder while a renderer is processed. Keeping
    /// this allocation-free matters on the per-frame GPU path.
    Vacant,
    Software(SoftwareRenderer),
    Vulkan(GpuRenderer),
    /// Software is used only until Rune has prepared this surface and exposed
    /// its device/queue. This avoids delaying first playback behind the UI's
    /// initial GPU frame.
    GpuNativePending(SoftwareRenderer),
    GpuNative(NativeGpuRenderer),
    /// Direct NV12 handoff waits for the same lazily-created Rune context as
    /// Mode B, then keeps Y/UV textures in Rune's compositor instead of
    /// rendering an intermediate BGRA texture.
    GpuVideoPending(SoftwareRenderer),
    GpuVideo(NativeVideoRenderer),
}

impl VideoRenderer {
    pub(crate) fn new() -> Self {
        match std::env::var("VEYRA_VIDEO_RENDERER").as_deref() {
            // The current Vulkan route must read BGRA back to the CPU before
            // Rune uploads it again. Benchmarks on 768x576 H.264 show that this
            // costs more CPU and RSS than swscale, so it remains opt-in until
            // Rune can consume a shared GPU texture.
            Ok("gpu-native") => {
                eprintln!("[video] renderer=gpu-native (awaiting Rune GPU context)");
                Self::GpuNativePending(SoftwareRenderer::new())
            }
            Ok("gpu-video") => {
                eprintln!("[video] renderer=gpu-video (awaiting Rune GPU context)");
                Self::GpuVideoPending(SoftwareRenderer::new())
            }
            // Keep `vulkan` working for existing launch scripts. It is the
            // diagnostic/readback implementation, not the shared-texture path.
            Ok("vulkan") | Ok("vulkan-readback") => match GpuRenderer::new() {
                Ok(renderer) => Self::Vulkan(renderer),
                Err(error) => {
                    eprintln!(
                        "[video] requested Vulkan renderer unavailable; falling back to software: {error}"
                    );
                    Self::Software(SoftwareRenderer::new())
                }
            },
            Ok("software") => {
                eprintln!(
                    "[video] renderer=software (explicit fallback; gpu-video is the default)"
                );
                Self::Software(SoftwareRenderer::new())
            }
            Err(_) => {
                // Direct Y/UV handoff avoids a full CPU BGRA frame and its
                // subsequent upload. Unsupported formats and GPU failures
                // already fall back locally to the software renderer.
                eprintln!("[video] renderer=gpu-video (default; awaiting Rune GPU context)");
                Self::GpuVideoPending(SoftwareRenderer::new())
            }
            Ok(value) => {
                eprintln!(
                    "[video] unknown VEYRA_VIDEO_RENDERER={value:?}; using gpu-video (valid: software, gpu-video, gpu-native, vulkan-readback)"
                );
                Self::GpuVideoPending(SoftwareRenderer::new())
            }
        }
    }

    pub(crate) fn set_gpu_context(&mut self, context: GpuContext) {
        let current = std::mem::replace(self, Self::Vacant);
        match current {
            Self::GpuNativePending(_) => match NativeGpuRenderer::new(context) {
                Ok(renderer) => {
                    eprintln!("[video] gpu-native renderer attached to Rune device");
                    *self = Self::GpuNative(renderer);
                }
                Err(error) => {
                    eprintln!("[video] gpu-native unavailable; using software fallback: {error}");
                    *self = Self::Software(SoftwareRenderer::new());
                }
            },
            Self::GpuNative(renderer) if renderer.generation() != context.generation => {
                match NativeGpuRenderer::new(context) {
                    Ok(renderer) => {
                        eprintln!(
                            "[video] gpu-native renderer recreated for Rune device generation"
                        );
                        *self = Self::GpuNative(renderer);
                    }
                    Err(error) => {
                        eprintln!(
                            "[video] replacement Rune GPU unavailable; using software fallback: {error}"
                        );
                        *self = Self::Software(SoftwareRenderer::new());
                    }
                }
            }
            Self::GpuVideoPending(_) => match NativeVideoRenderer::new(context) {
                Ok(renderer) => {
                    eprintln!("[video] gpu-video renderer attached to Rune device");
                    *self = Self::GpuVideo(renderer);
                }
                Err(error) => {
                    eprintln!("[video] gpu-video unavailable; using software fallback: {error}");
                    *self = Self::Software(SoftwareRenderer::new());
                }
            },
            Self::GpuVideo(renderer) if renderer.generation() != context.generation => {
                match NativeVideoRenderer::new(context) {
                    Ok(renderer) => {
                        eprintln!(
                            "[video] gpu-video renderer recreated for Rune device generation"
                        );
                        *self = Self::GpuVideo(renderer);
                    }
                    Err(error) => {
                        eprintln!(
                            "[video] replacement Rune GPU unavailable; using software fallback: {error}"
                        );
                        *self = Self::Software(SoftwareRenderer::new());
                    }
                }
            }
            Self::Vacant => unreachable!("renderer is only vacant while being replaced"),
            renderer => *self = renderer,
        }
    }

    pub(crate) unsafe fn convert(
        &mut self,
        frame: *mut ffi::AVFrame,
        timestamp: f64,
    ) -> Result<Option<VideoFrame>, String> {
        // Keep fallback local to the renderer: an unusual pixel format or a
        // device loss must not stop playback or tear down the decoder.
        let current = std::mem::replace(self, Self::Vacant);
        match current {
            Self::Vacant => unreachable!("renderer is only vacant while converting a frame"),
            Self::Software(mut renderer) => {
                let result = renderer.convert(frame);
                *self = Self::Software(renderer);
                result.map(|(rgba, width, height)| {
                    Some(VideoFrame::Bytes {
                        rgba,
                        width,
                        height,
                        timestamp,
                    })
                })
            }
            Self::Vulkan(mut renderer)
                if (*frame).format == ffi::AVPixelFormat_AV_PIX_FMT_YUV420P =>
            {
                match renderer.render_yuv420p(frame, timestamp) {
                    Ok(result) => {
                        *self = Self::Vulkan(renderer);
                        Ok(result)
                    }
                    Err(error) => {
                        eprintln!("[video] Vulkan frame failed; falling back to software: {error}");
                        let mut software = SoftwareRenderer::new();
                        let result = software.convert(frame);
                        *self = Self::Software(software);
                        result.map(|(rgba, width, height)| {
                            Some(VideoFrame::Bytes {
                                rgba,
                                width,
                                height,
                                timestamp,
                            })
                        })
                    }
                }
            }
            Self::GpuNativePending(mut renderer) => {
                // The first UI draw publishes the Rune context. Until then,
                // retain normal software presentation rather than queueing or
                // dropping decoded frames.
                let result = renderer.convert(frame);
                *self = Self::GpuNativePending(renderer);
                result.map(|(rgba, width, height)| {
                    Some(VideoFrame::Bytes {
                        rgba,
                        width,
                        height,
                        timestamp,
                    })
                })
            }
            Self::GpuVideoPending(mut renderer) => {
                let result = renderer.convert(frame);
                *self = Self::GpuVideoPending(renderer);
                result.map(|(rgba, width, height)| {
                    Some(VideoFrame::Bytes {
                        rgba,
                        width,
                        height,
                        timestamp,
                    })
                })
            }
            Self::GpuNative(mut renderer)
                if (*frame).format == ffi::AVPixelFormat_AV_PIX_FMT_YUV420P =>
            {
                match renderer.render_yuv420p(frame, timestamp) {
                    Ok(result) => {
                        *self = Self::GpuNative(renderer);
                        Ok(result)
                    }
                    Err(error) => {
                        eprintln!(
                            "[video] gpu-native frame failed; falling back to software: {error}"
                        );
                        let mut software = SoftwareRenderer::new();
                        let result = software.convert(frame);
                        *self = Self::Software(software);
                        result.map(|(rgba, width, height)| {
                            Some(VideoFrame::Bytes {
                                rgba,
                                width,
                                height,
                                timestamp,
                            })
                        })
                    }
                }
            }
            Self::GpuNative(_) => {
                eprintln!(
                    "[video] unsupported pixel format for gpu-native renderer; using software fallback"
                );
                let mut software = SoftwareRenderer::new();
                let result = software.convert(frame);
                *self = Self::Software(software);
                result.map(|(rgba, width, height)| {
                    Some(VideoFrame::Bytes {
                        rgba,
                        width,
                        height,
                        timestamp,
                    })
                })
            }
            Self::GpuVideo(mut renderer)
                if matches!(
                    (*frame).format,
                    ffi::AVPixelFormat_AV_PIX_FMT_YUV420P | ffi::AVPixelFormat_AV_PIX_FMT_NV12
                ) =>
            {
                match renderer.upload(frame, timestamp) {
                    Ok(result) => {
                        *self = Self::GpuVideo(renderer);
                        Ok(result)
                    }
                    Err(error) => {
                        eprintln!(
                            "[video] gpu-video frame failed; falling back to software: {error}"
                        );
                        let mut software = SoftwareRenderer::new();
                        let result = software.convert(frame);
                        *self = Self::Software(software);
                        result.map(|(rgba, width, height)| {
                            Some(VideoFrame::Bytes {
                                rgba,
                                width,
                                height,
                                timestamp,
                            })
                        })
                    }
                }
            }
            Self::GpuVideo(_) => {
                eprintln!(
                    "[video] unsupported pixel format for gpu-video renderer; using software fallback"
                );
                let mut software = SoftwareRenderer::new();
                let result = software.convert(frame);
                *self = Self::Software(software);
                result.map(|(rgba, width, height)| {
                    Some(VideoFrame::Bytes {
                        rgba,
                        width,
                        height,
                        timestamp,
                    })
                })
            }
            Self::Vulkan(_) => {
                eprintln!(
                    "[video] unsupported pixel format for Vulkan renderer; using software fallback"
                );
                let mut software = SoftwareRenderer::new();
                let result = software.convert(frame);
                *self = Self::Software(software);
                result.map(|(rgba, width, height)| {
                    Some(VideoFrame::Bytes {
                        rgba,
                        width,
                        height,
                        timestamp,
                    })
                })
            }
        }
    }

    pub(crate) fn poll_completed(&mut self) -> Result<Option<VideoFrame>, String> {
        match self {
            Self::Vacant => unreachable!("renderer is only vacant while converting a frame"),
            Self::Software(_) => Ok(None),
            Self::Vulkan(renderer) => renderer.poll_completed(),
            Self::GpuNativePending(_)
            | Self::GpuNative(_)
            | Self::GpuVideoPending(_)
            | Self::GpuVideo(_) => Ok(None),
        }
    }

    pub(crate) fn finish_pending(&mut self) -> Result<Option<VideoFrame>, String> {
        match self {
            Self::Vacant => unreachable!("renderer is only vacant while converting a frame"),
            Self::Software(_) => Ok(None),
            Self::Vulkan(renderer) => renderer.finish_pending(),
            Self::GpuNativePending(_)
            | Self::GpuNative(_)
            | Self::GpuVideoPending(_)
            | Self::GpuVideo(_) => Ok(None),
        }
    }

    pub(crate) fn reset_pending(&mut self) {
        match self {
            Self::Vulkan(renderer) => renderer.reset_pending(),
            Self::Vacant => unreachable!("renderer is only vacant while converting a frame"),
            _ => {}
        }
    }
}
