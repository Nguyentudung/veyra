use super::super::ffi;
use std::path::Path;

pub(crate) struct Demuxer {
    pub(crate) format: *mut ffi::AVFormatContext,
    pub(crate) packet: *mut ffi::AVPacket,
}

impl Demuxer {
    pub(crate) unsafe fn open(path: &Path) -> Result<Self, String> {
        let mut format = std::ptr::null_mut();
        let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| "invalid path")?;
        check(
            ffi::avformat_open_input(
                &mut format,
                c_path.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ),
            "open container",
        )?;
        if ffi::avformat_find_stream_info(format, std::ptr::null_mut()) < 0 {
            ffi::avformat_close_input(&mut format);
            return Err("could not read media streams".into());
        }
        let packet = ffi::av_packet_alloc();
        if packet.is_null() {
            ffi::avformat_close_input(&mut format);
            return Err("could not allocate packet".into());
        }
        Ok(Self { format, packet })
    }
    pub(crate) unsafe fn read_packet(&mut self) -> Result<i32, String> {
        if ffi::av_read_frame(self.format, self.packet) < 0 {
            Err("playback finished".into())
        } else {
            Ok((*self.packet).stream_index)
        }
    }
}

impl Drop for Demuxer {
    fn drop(&mut self) {
        unsafe {
            if !self.packet.is_null() {
                ffi::av_packet_free(&mut self.packet);
            }
            if !self.format.is_null() {
                ffi::avformat_close_input(&mut self.format);
            }
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
