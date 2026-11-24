use super::super::ffi;

// These flags are stable FFmpeg API values. bindgen does not expose them
// because FF_THREAD_* is an enum defined only in a codec-internal header.
const FF_THREAD_FRAME: i32 = 1;
const FF_THREAD_SLICE: i32 = 2;
const FF_THREAD_ALL: i32 = FF_THREAD_FRAME | FF_THREAD_SLICE;
const THREAD_CAPABILITIES: u32 = ffi::AV_CODEC_CAP_FRAME_THREADS | ffi::AV_CODEC_CAP_SLICE_THREADS;
const DEFAULT_MAX_VIDEO_THREADS: usize = 8;

pub(crate) struct Codec {
    pub(crate) context: *mut ffi::AVCodecContext,
    pub(crate) stream_index: i32,
    pub(crate) time_base: ffi::AVRational,
    pub(crate) frame_rate: Option<f64>,
    /// The values FFmpeg accepted after opening the video decoder. Audio
    /// codecs intentionally retain FFmpeg's single-thread defaults: extra
    /// decoder workers tend to add overhead without improving playback.
    pub(crate) decode_threads: Option<u32>,
    pub(crate) decode_thread_type: Option<String>,
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
    // FFmpeg requires packet PTS/DTS time base to be supplied by decoding
    // callers. This keeps frame timestamps stable when frame threading delays
    // or reorders decoder output.
    (*context).pkt_timebase = (*stream).time_base;
    let requested_threading = (media_type == ffi::AVMediaType_AVMEDIA_TYPE_VIDEO as i32)
        .then(|| DecodeThreading::from_environment((*codec_ptr).capabilities as u32));
    if let Some(threading) = requested_threading {
        // FFmpeg creates its own worker pool when avcodec_open2 runs. Keeping
        // this configuration at the codec boundary preserves packet/frame
        // ordering and avoids unsafely splitting AVCodecContext ownership
        // across Rust threads.
        (*context).thread_count = threading.count;
        (*context).thread_type = threading.thread_type;
    }
    check(
        ffi::avcodec_open2(context, codec_ptr, std::ptr::null_mut()),
        "open decoder",
    )?;
    let (decode_threads, decode_thread_type) = if requested_threading.is_some() {
        let active_type = (*context).active_thread_type;
        // Some codecs advertise neither frame nor slice threading. FFmpeg
        // keeps the requested count in that case, but executes serially; show
        // telemetry consumers the effective single-threaded mode instead.
        let count = if active_type == 0 {
            1
        } else {
            (*context).thread_count.max(1) as u32
        };
        let label = thread_type_label(active_type).to_owned();
        eprintln!(
            "[video] decoder threads={count} mode={label} (requested {})",
            requested_threading
                .expect("video threading was configured")
                .description(),
        );
        (Some(count), Some(label))
    } else {
        (None, None)
    };
    Ok(Some(Codec {
        context,
        stream_index: index,
        time_base: (*stream).time_base,
        frame_rate: ((*stream).avg_frame_rate.den > 0 && (*stream).avg_frame_rate.num > 0)
            .then_some((*stream).avg_frame_rate.num as f64 / (*stream).avg_frame_rate.den as f64),
        decode_threads,
        decode_thread_type,
    }))
}

/// Requested FFmpeg threading policy for one video codec. `auto` uses all
/// supported thread types and leaves one logical CPU available to Rune, audio,
/// and the operating system. A user can pin a count or choose frame/slice
/// threading with the environment variables documented in `docs/performance.md`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DecodeThreading {
    count: i32,
    thread_type: i32,
}

impl DecodeThreading {
    fn from_environment(capabilities: u32) -> Self {
        let available = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1);
        let count = resolve_thread_count(
            std::env::var("VEYRA_VIDEO_THREADS").ok().as_deref(),
            available,
        );
        let mode = ThreadMode::parse(std::env::var("VEYRA_VIDEO_THREAD_MODE").ok().as_deref());
        Self {
            count: count as i32,
            thread_type: resolve_thread_type(mode, capabilities, count),
        }
    }

    fn description(self) -> String {
        format!("{} {}", self.count, thread_type_label(self.thread_type))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThreadMode {
    Auto,
    Frame,
    Slice,
}

impl ThreadMode {
    fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some(value) if value.eq_ignore_ascii_case("frame") => Self::Frame,
            Some(value) if value.eq_ignore_ascii_case("slice") => Self::Slice,
            _ => Self::Auto,
        }
    }
}

fn resolve_thread_count(value: Option<&str>, available: usize) -> usize {
    let available = available.max(1);
    let automatic = available
        .saturating_sub(1)
        .clamp(1, DEFAULT_MAX_VIDEO_THREADS);
    match value.map(str::trim) {
        None | Some("") | Some("auto") | Some("0") => automatic,
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|count| *count > 0)
            .map(|count| count.min(available))
            .unwrap_or(automatic),
    }
}

fn resolve_thread_type(mode: ThreadMode, capabilities: u32, count: usize) -> i32 {
    if count <= 1 {
        return 0;
    }
    let supported = capabilities & THREAD_CAPABILITIES;
    let frame_supported = supported & ffi::AV_CODEC_CAP_FRAME_THREADS != 0;
    let slice_supported = supported & ffi::AV_CODEC_CAP_SLICE_THREADS != 0;
    match mode {
        // Asking FFmpeg to consider every type it supports gives it the best
        // choice for the active codec and stream. Explicit modes remain useful
        // for latency-vs-throughput experiments.
        ThreadMode::Auto => {
            (frame_supported as i32 * FF_THREAD_FRAME) | (slice_supported as i32 * FF_THREAD_SLICE)
        }
        ThreadMode::Frame if frame_supported => FF_THREAD_FRAME,
        ThreadMode::Slice if slice_supported => FF_THREAD_SLICE,
        // Do not make an otherwise valid file fail just because a chosen mode
        // is unavailable for its codec. Fall back to FFmpeg's supported set.
        ThreadMode::Frame | ThreadMode::Slice => {
            (frame_supported as i32 * FF_THREAD_FRAME) | (slice_supported as i32 * FF_THREAD_SLICE)
        }
    }
}

fn thread_type_label(thread_type: i32) -> &'static str {
    match thread_type & FF_THREAD_ALL {
        FF_THREAD_FRAME => "frame",
        FF_THREAD_SLICE => "slice",
        FF_THREAD_ALL => "frame+slice",
        _ => "single",
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_thread_count_keeps_a_cpu_for_the_app() {
        assert_eq!(resolve_thread_count(None, 1), 1);
        assert_eq!(resolve_thread_count(Some("auto"), 2), 1);
        assert_eq!(resolve_thread_count(Some("0"), 8), 7);
        assert_eq!(resolve_thread_count(None, 32), DEFAULT_MAX_VIDEO_THREADS);
    }

    #[test]
    fn explicit_thread_count_is_bounded_by_available_parallelism() {
        assert_eq!(resolve_thread_count(Some("1"), 16), 1);
        assert_eq!(resolve_thread_count(Some("12"), 4), 4);
        assert_eq!(resolve_thread_count(Some("not-a-number"), 4), 3);
    }

    #[test]
    fn mode_only_requests_capabilities_the_codec_has() {
        let frame = ffi::AV_CODEC_CAP_FRAME_THREADS;
        let slice = ffi::AV_CODEC_CAP_SLICE_THREADS;
        assert_eq!(resolve_thread_type(ThreadMode::Auto, frame | slice, 4), 3);
        assert_eq!(resolve_thread_type(ThreadMode::Frame, slice, 4), 2);
        assert_eq!(resolve_thread_type(ThreadMode::Slice, frame, 4), 1);
        assert_eq!(resolve_thread_type(ThreadMode::Auto, frame, 1), 0);
    }
}
