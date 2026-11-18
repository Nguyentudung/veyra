use super::super::ffi;

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
