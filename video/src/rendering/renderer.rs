use super::super::ffi;
use super::frame::VideoFrame;
use super::gpu::GpuRenderer;

pub(crate) struct SoftwareRenderer {
    context: *mut ffi::SwsContext,
}

impl SoftwareRenderer {
    pub(crate) fn new() -> Self {
        Self {
            context: std::ptr::null_mut(),
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
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        let mut data = [
            rgba.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ];
        let lines = [width * 4, 0, 0, 0];
        ffi::sws_scale(
            self.context,
            (*frame).data.as_ptr() as *const *const u8,
            (*frame).linesize.as_ptr(),
            0,
            height,
            data.as_mut_ptr(),
            lines.as_ptr(),
        );
        Ok((rgba, width as u32, height as u32))
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
    Software(SoftwareRenderer),
    Vulkan(GpuRenderer),
}

impl VideoRenderer {
    pub(crate) fn new() -> Self {
        match std::env::var("VEYRA_VIDEO_RENDERER").as_deref() {
            // The current Vulkan route must read BGRA back to the CPU before
            // Rune uploads it again. Benchmarks on 768x576 H.264 show that this
            // costs more CPU and RSS than swscale, so it remains opt-in until
            // Rune can consume a shared GPU texture.
            Ok("vulkan") => match GpuRenderer::new() {
                Ok(renderer) => Self::Vulkan(renderer),
                Err(error) => {
                    eprintln!(
                        "[video] requested Vulkan renderer unavailable; falling back to software: {error}"
                    );
                    Self::Software(SoftwareRenderer::new())
                }
            },
            Ok("software") | Err(_) => {
                eprintln!(
                    "[video] renderer=software (default; set VEYRA_VIDEO_RENDERER=vulkan to opt in)"
                );
                Self::Software(SoftwareRenderer::new())
            }
            Ok(value) => {
                eprintln!(
                    "[video] unknown VEYRA_VIDEO_RENDERER={value:?}; using software (set it to `vulkan` to opt in)"
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
        // Keep fallback local to the renderer: an unusual pixel format or a
        // device loss must not stop playback or tear down the decoder.
        let current = std::mem::replace(self, Self::Software(SoftwareRenderer::new()));
        match current {
            Self::Software(mut renderer) => {
                let result = renderer.convert(frame);
                *self = Self::Software(renderer);
                result.map(|(rgba, width, height)| {
                    Some(VideoFrame {
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
                            Some(VideoFrame {
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
                eprintln!(
                    "[video] unsupported pixel format for Vulkan renderer; using software fallback"
                );
                let mut software = SoftwareRenderer::new();
                let result = software.convert(frame);
                *self = Self::Software(software);
                result.map(|(rgba, width, height)| {
                    Some(VideoFrame {
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
            Self::Software(_) => Ok(None),
            Self::Vulkan(renderer) => renderer.poll_completed(),
        }
    }

    pub(crate) fn finish_pending(&mut self) -> Result<Option<VideoFrame>, String> {
        match self {
            Self::Software(_) => Ok(None),
            Self::Vulkan(renderer) => renderer.finish_pending(),
        }
    }

    pub(crate) fn reset_pending(&mut self) {
        if let Self::Vulkan(renderer) = self {
            renderer.reset_pending();
        }
    }
}
