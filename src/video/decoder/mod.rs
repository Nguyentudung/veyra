mod codec;
mod demuxer;
mod seek;

use super::{
    audio::{self, AudioOutput},
    rendering::{self, VideoFrame},
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
    renderer: super::rendering::SoftwareRenderer,
    presentation_clock: rendering::PresentationClock,
}

unsafe impl Send for Decoder {}

impl Decoder {
    pub(crate) fn open(path: &std::path::Path) -> Result<Self, String> {
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
            Ok(Self {
                demuxer,
                audio,
                video,
                frame,
                audio_output: None,
                renderer: super::rendering::SoftwareRenderer::new(),
                presentation_clock: rendering::PresentationClock::new(),
            })
        }
    }

    pub(crate) fn step(
        &mut self,
        snapshot: &Arc<Mutex<Snapshot>>,
        frame_out: &Arc<Mutex<Option<VideoFrame>>>,
    ) -> Result<(), String> {
        unsafe {
            let stream = self.demuxer.read_packet()?;
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
        snapshot.lock().unwrap().position = target;
        Ok(())
    }

    pub(crate) fn seek_to(
        &mut self,
        target: f64,
        snapshot: &Arc<Mutex<Snapshot>>,
        frame_out: &Arc<Mutex<Option<VideoFrame>>>,
    ) -> Result<(), String> {
        let duration = snapshot.lock().unwrap().duration;
        let target = target.max(0.0).min(duration.unwrap_or(f64::MAX));
        self.seek_absolute(target)?;
        self.presentation_clock.reset();
        *frame_out.lock().unwrap() = None;
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

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            if !self.frame.is_null() {
                super::ffi::av_frame_free(&mut self.frame);
            }
        }
    }
}
