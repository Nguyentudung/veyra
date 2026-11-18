use super::super::ffi;

pub(super) unsafe fn create(
    sample_format: i32,
    rate: i32,
    channels: u32,
) -> Result<*mut ffi::SwrContext, String> {
    let mut swr = std::ptr::null_mut();
    let mut input_layout = std::mem::zeroed();
    ffi::av_channel_layout_default(&mut input_layout, channels as i32);
    let mut output_layout = std::mem::zeroed();
    ffi::av_channel_layout_default(&mut output_layout, 2);
    check(
        ffi::swr_alloc_set_opts2(
            &mut swr,
            &output_layout,
            ffi::AVSampleFormat_AV_SAMPLE_FMT_S16,
            48_000,
            &input_layout,
            sample_format,
            rate,
            0,
            std::ptr::null_mut(),
        ),
        "configure resampler",
    )?;
    check(ffi::swr_init(swr), "initialize resampler")?;
    Ok(swr)
}

fn check(code: i32, operation: &str) -> Result<(), String> {
    if code < 0 {
        Err(format!("{operation} failed ({code})"))
    } else {
        Ok(())
    }
}
