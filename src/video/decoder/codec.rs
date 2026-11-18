use super::super::ffi;

pub(crate) struct Codec {
    pub(crate) context: *mut ffi::AVCodecContext,
    pub(crate) stream_index: i32,
    pub(crate) time_base: ffi::AVRational,
    pub(crate) frame_rate: Option<f64>,
}

pub(crate) unsafe fn open(
    format: *mut ffi::AVFormatContext,
    media_type: i32,
) -> Result<Option<Codec>, String> {
    let mut codec_ptr = std::ptr::null();
    let index = ffi::av_find_best_stream(format, media_type, -1, -1, &mut codec_ptr, 0);
    if index < 0 {
        return Ok(None);
    }
    let context = ffi::avcodec_alloc_context3(codec_ptr);
    if context.is_null() {
        return Err("could not allocate codec context".into());
    }
    let stream = *(*format).streams.add(index as usize);
    check(
        ffi::avcodec_parameters_to_context(context, (*stream).codecpar),
        "copy codec parameters",
    )?;
    check(
        ffi::avcodec_open2(context, codec_ptr, std::ptr::null_mut()),
        "open decoder",
    )?;
    Ok(Some(Codec {
        context,
        stream_index: index,
        time_base: (*stream).time_base,
        frame_rate: ((*stream).avg_frame_rate.den > 0 && (*stream).avg_frame_rate.num > 0)
            .then_some((*stream).avg_frame_rate.num as f64 / (*stream).avg_frame_rate.den as f64),
    }))
}

impl Drop for Codec {
    fn drop(&mut self) {
        unsafe {
            let mut context = self.context;
            ffi::avcodec_free_context(&mut context);
        }
    }
}
fn check(code: i32, operation: &str) -> Result<(), String> {
    if code < 0 {
        Err(format!("{operation} failed ({code})"))
    } else {
        Ok(())
    }
}
