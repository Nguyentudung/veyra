use super::super::ffi;
use super::frame::VideoFrame;
use super::gpu::GpuRenderer;
use std::mem::MaybeUninit;

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
        // sws_scale overwrites every visible output pixel. Allocating an
        // already-zeroed 4K BGRA frame first made the allocator touch tens of
        // MiB per second only for the scaler to overwrite it immediately.
        // Keep the allocation explicitly uninitialised until FFmpeg has
        // reported a full-height conversion; padding is never read.
        let allocation_bytes = scaled_bytes
            .checked_add(self.output_guard_bytes)
            .ok_or("software frame allocation size overflow")?;
        let mut rgba = Vec::<MaybeUninit<u8>>::with_capacity(allocation_bytes);
        // Safety: bytes are either written by sws_scale before being read, or
        // remain MaybeUninit and are never exposed as u8. The optional guard
        // is initialized explicitly below.
        unsafe { rgba.set_len(allocation_bytes) };
        if self.output_guard_bytes != 0 {
            // Use a non-zero pattern: a zero-filled guard could fail to detect
            // an overrun made solely of zero-valued color components.
            for byte in &mut rgba[scaled_bytes..] {
                byte.write(0xA5);
            }
        }
        let mut data = [
            rgba.as_mut_ptr().cast::<u8>(),
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
            && let Some(first_changed) = guard
                .iter()
                .position(|byte| unsafe { byte.assume_init() } != 0xA5)
        {
            let last_changed = guard
                .iter()
                .rposition(|byte| unsafe { byte.assume_init() } != 0xA5)
                .unwrap_or(first_changed);
            let changed_bytes = guard
                .iter()
                .filter(|byte| unsafe { byte.assume_init() } != 0xA5)
                .count();
            return Err(format!(
                "sws_scale wrote beyond the {scaled_bytes}-byte BGRA destination (guard changed: {changed_bytes} bytes, first: {first_changed}, last: {last_changed}, guard: {} bytes)",
                self.output_guard_bytes,
            ));
        }
        // The BGRA frame path takes tightly packed rows. Copy
        // away only the alignment padding that libswscale needs internally.
        if output_stride == tight_stride {
            rgba.truncate(image_bytes);
            // The visible output is fully initialized after a successful
            // full-height sws_scale call. MaybeUninit<u8> has the same layout
            // as u8, so transfer this allocation without a second copy.
            let capacity = rgba.capacity();
            let length = rgba.len();
            let pointer = rgba.as_mut_ptr().cast::<u8>();
            std::mem::forget(rgba);
            let rgba = unsafe { Vec::from_raw_parts(pointer, length, capacity) };
            return Ok((rgba, width as u32, height as u32));
        }
        let mut packed = vec![0u8; image_bytes];
        for row in 0..height {
            let source_start = row * output_stride;
            let target_start = row * tight_stride;
            // sws_scale initialized every visible row above. Padding remains
            // MaybeUninit and is deliberately excluded from this slice.
            let source = unsafe {
                std::slice::from_raw_parts(
                    rgba.as_ptr().add(source_start).cast::<u8>(),
                    tight_stride,
                )
            };
            packed[target_start..target_start + tight_stride].copy_from_slice(source);
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
    Vacant,
    Software(SoftwareRenderer),
    Vulkan(GpuRenderer),
}

impl VideoRenderer {
    pub(crate) fn new() -> Self {
        match std::env::var("VEYRA_VIDEO_RENDERER").as_deref() {
            Ok("vulkan") | Ok("vulkan-readback") => match GpuRenderer::new() {
                Ok(renderer) => Self::Vulkan(renderer),
                Err(error) => {
                    eprintln!("[video] Vulkan unavailable; using software: {error}");
                    Self::Software(SoftwareRenderer::new())
                }
            },
            Ok("software") | Err(_) => Self::Software(SoftwareRenderer::new()),
            Ok(value) => {
                eprintln!(
                    "[video] renderer {value:?} is unavailable; using software (supported: software, vulkan-readback)"
                );
                Self::Software(SoftwareRenderer::new())
            }
        }
    }

    pub(crate) unsafe fn convert(
        &mut self,
        frame: *mut ffi::AVFrame,
        timestamp: f64,
    ) -> Result<Option<VideoFrame>, String> {
        let current = std::mem::replace(self, Self::Vacant);
        match current {
            Self::Vacant => unreachable!("renderer is only vacant during conversion"),
            Self::Software(mut renderer) => {
                let result = unsafe { renderer.convert(frame) };
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
                if unsafe { (*frame).format == ffi::AVPixelFormat_AV_PIX_FMT_YUV420P } =>
            {
                match unsafe { renderer.render_yuv420p(frame, timestamp) } {
                    Ok(result) => {
                        *self = Self::Vulkan(renderer);
                        Ok(result)
                    }
                    Err(error) => {
                        eprintln!("[video] Vulkan frame failed; using software: {error}");
                        let mut software = SoftwareRenderer::new();
                        let result = unsafe { software.convert(frame) };
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
            Self::Vulkan(_) => {
                eprintln!("[video] unsupported Vulkan pixel format; using software");
                let mut software = SoftwareRenderer::new();
                let result = unsafe { software.convert(frame) };
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
            Self::Vulkan(renderer) => renderer.poll_completed(),
            Self::Software(_) => Ok(None),
            Self::Vacant => unreachable!("renderer is only vacant during conversion"),
        }
    }

    pub(crate) fn finish_pending(&mut self) -> Result<Option<VideoFrame>, String> {
        match self {
            Self::Vulkan(renderer) => renderer.finish_pending(),
            Self::Software(_) => Ok(None),
            Self::Vacant => unreachable!("renderer is only vacant during conversion"),
        }
    }

    pub(crate) fn reset_pending(&mut self) {
        if let Self::Vulkan(renderer) = self {
            renderer.reset_pending();
        }
    }
}
