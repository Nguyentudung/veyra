mod codec;
mod demuxer;
mod seek;

use super::{
    FrameMailbox,
    audio::{self, AudioOutput},
    rendering,
    state::Snapshot,
};
use codec::Codec;
use demuxer::Demuxer;
use std::sync::{Arc, Mutex};

pub(crate) struct Decoder {
    demuxer: Demuxer,
    audio: Option<Codec>,
    video: Option<Codec>,
    frame: *mut super::ffi::AVFrame,
    audio_output: Option<AudioOutput>,
    renderer: super::rendering::VideoRenderer,
    presentation_clock: rendering::PresentationClock,
    initial_state: Snapshot,
}

unsafe impl Send for Decoder {}

impl Decoder {
    pub(crate) fn open(
        path: &std::path::Path,
        gpu_context: Option<&crate::GpuContext>,
    ) -> Result<Self, String> {
        unsafe {
            let demuxer = Demuxer::open(path)?;
            let audio = codec::open(demuxer.format, 1)?;
            let video = codec::open(demuxer.format, 0)?;
            if audio.is_none() && video.is_none() {
                return Err("media has no decodable audio or video stream".into());
            }
            let frame = super::ffi::av_frame_alloc();
            if frame.is_null() {
                return Err("could not allocate decoder frame".into());
            }
            let file_size = std::fs::metadata(path).ok().map(|m| m.len());
            let mut state = Snapshot::default();
            state.file_name = path.file_name().and_then(|v| v.to_str()).map(str::to_owned);
            state.file_path = Some(path.display().to_string());
            state.container = path
                .extension()
                .and_then(|v| v.to_str())
                .map(|v| v.to_ascii_lowercase());
            state.file_size = file_size;
            if let Some(video) = video.as_ref() {
                state.video_codec = codec_name(video.context);
                state.video_fps = video.frame_rate;
                state.video_bitrate =
                    ((*video.context).bit_rate > 0).then_some((*video.context).bit_rate as u64);
                state.video_profile = codec_profile(video.context);
                state.video_pixel_format = pixel_format(video.context);
                state.video_decoder = Some("FFmpeg software decoder".into());
            }
            if let Some(audio) = audio.as_ref() {
                state.audio_codec = codec_name(audio.context);
                state.audio_sample_rate = ((*audio.context).sample_rate > 0)
                    .then_some((*audio.context).sample_rate as u32);
                state.audio_channels = ((*audio.context).ch_layout.nb_channels > 0)
                    .then_some((*audio.context).ch_layout.nb_channels as u32);
                state.audio_bitrate =
                    ((*audio.context).bit_rate > 0).then_some((*audio.context).bit_rate as u64);
                state.audio_output = Some("ALSA default".into());
            }
            let mut renderer = super::rendering::VideoRenderer::new();
            if let Some(context) = gpu_context {
                renderer.set_gpu_context(context.clone());
            }
            Ok(Self {
                demuxer,
                audio,
                video,
                frame,
                audio_output: None,
                renderer,
                presentation_clock: rendering::PresentationClock::new(),
                initial_state: state,
            })
        }
    }

    pub(crate) fn metadata(&self) -> Snapshot {
        self.initial_state.clone()
    }

    pub(crate) fn reset_presentation_clock(&mut self) {
        self.presentation_clock.reset();
    }

    pub(crate) fn set_gpu_context(&mut self, context: crate::GpuContext) {
        self.renderer.set_gpu_context(context);
    }

    pub(crate) fn pause_audio(&mut self) {
        if let Some(output) = self.audio_output.as_mut() {
            output.pause();
        }
    }

    pub(crate) fn resume_audio(&mut self) {
        if let Some(output) = self.audio_output.as_mut() {
            output.resume();
        }
    }

    pub(crate) fn step(
        &mut self,
        snapshot: &Arc<Mutex<Snapshot>>,
        frame_out: &Arc<FrameMailbox>,
    ) -> Result<(), String> {
        unsafe {
            let stream = match self.demuxer.read_packet() {
                Ok(stream) => stream,
                Err(error) if error == "playback finished" => {
                    rendering::finish_pending(&mut self.renderer, frame_out, snapshot)?;
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            let packet = self.demuxer.packet;
            if self.video.as_ref().map(|codec| codec.stream_index) == Some(stream) {
                if let Some(codec) = self.video.as_ref() {
                    rendering::decode_video(
                        codec.context,
                        packet,
                        self.frame,
                        &mut self.renderer,
                        frame_out,
                        snapshot,
                        codec.time_base,
                        &mut self.presentation_clock,
                    )?;
                }
            } else if self.audio.as_ref().map(|codec| codec.stream_index) == Some(stream) {
                if let Some(codec) = self.audio.as_ref() {
                    audio::decode_audio(
                        codec.context,
                        packet,
                        self.frame,
                        &mut self.audio_output,
                        snapshot,
                    )?;
                }
            }
            super::ffi::av_packet_unref(packet);
            rendering::publish_completed(&mut self.renderer, frame_out, snapshot)?;
            if let Some(video) = self.video.as_ref() {
                let mut state = snapshot.lock().unwrap();
                state.video_width = (*video.context).width as u32;
                state.video_height = (*video.context).height as u32;
            }
            Ok(())
        }
    }

    pub(crate) fn seek(
        &mut self,
        delta: f64,
        snapshot: &Arc<Mutex<Snapshot>>,
    ) -> Result<(), String> {
        let current = snapshot.lock().unwrap().position;
        let target = (current + delta).max(0.0);
        self.seek_absolute(target)?;
        self.presentation_clock.reset();
        self.renderer.reset_pending();
        snapshot.lock().unwrap().position = target;
        Ok(())
    }

    pub(crate) fn seek_to(
        &mut self,
        target: f64,
        snapshot: &Arc<Mutex<Snapshot>>,
        frame_out: &Arc<FrameMailbox>,
    ) -> Result<(), String> {
        let duration = snapshot.lock().unwrap().duration;
        let target = target.max(0.0).min(duration.unwrap_or(f64::MAX));
        self.seek_absolute(target)?;
        self.presentation_clock.reset();
        self.renderer.reset_pending();
        frame_out.clear();
        snapshot.lock().unwrap().position = target;
        Ok(())
    }

    fn seek_absolute(&mut self, target: f64) -> Result<(), String> {
        unsafe {
            seek::relative(
                self.demuxer.format,
                self.audio.as_ref(),
                self.video.as_ref(),
                target,
            )
        }?;
        // Seeking must discard already queued PCM so stale audio cannot play
        // against the reset video clock. The next decoded audio frame opens a
        // fresh ALSA stream; make this visible because it is expected on seek
        // but not during steady playback.
        if self.audio_output.is_some() {
            eprintln!("[audio] resetting output for seek to {target:.3}s");
        }
        self.audio_output = None;
        Ok(())
    }

    pub(crate) fn duration(&self) -> Option<f64> {
        unsafe {
            ((*self.demuxer.format).duration >= 0)
                .then_some((*self.demuxer.format).duration as f64 / 1_000_000.0)
        }
    }
}

fn codec_name(context: *mut super::ffi::AVCodecContext) -> Option<String> {
    unsafe {
        let ptr = super::ffi::avcodec_get_name((*context).codec_id);
        (!ptr.is_null()).then(|| std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned())
    }
}

fn codec_profile(context: *mut super::ffi::AVCodecContext) -> Option<String> {
    unsafe { ((*context).profile >= 0).then_some((*context).profile.to_string()) }
}

fn pixel_format(context: *mut super::ffi::AVCodecContext) -> Option<String> {
    unsafe { ((*context).pix_fmt >= 0).then_some(format!("pixel format {}", (*context).pix_fmt)) }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            if !self.frame.is_null() {
                super::ffi::av_frame_free(&mut self.frame);
            }
        }
    }
}
