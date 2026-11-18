use super::{super::ffi, codec::Codec};

pub(crate) unsafe fn relative(
    format: *mut ffi::AVFormatContext,
    audio: Option<&Codec>,
    video: Option<&Codec>,
    target: f64,
) -> Result<(), String> {
    ffi::avformat_flush(format);
    check(
        ffi::avformat_seek_file(
            format,
            -1,
            i64::MIN,
            (target * 1_000_000.0) as i64,
            i64::MAX,
            1,
        ),
        "seek",
    )?;
    if let Some(codec) = audio {
        ffi::avcodec_flush_buffers(codec.context);
    }
    if let Some(codec) = video {
        ffi::avcodec_flush_buffers(codec.context);
    }
    Ok(())
}
fn check(code: i32, operation: &str) -> Result<(), String> {
    if code < 0 {
        Err(format!("{operation} failed ({code})"))
    } else {
        Ok(())
    }
}
