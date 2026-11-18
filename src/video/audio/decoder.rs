use super::{
    super::{ffi, state::Snapshot},
    output::AudioOutput,
};
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

pub(crate) unsafe fn decode_audio(
    context: *mut ffi::AVCodecContext,
    packet: *mut ffi::AVPacket,
    frame: *mut ffi::AVFrame,
    output: &mut Option<AudioOutput>,
    snapshot: &Arc<Mutex<Snapshot>>,
) -> Result<(), String> {
    check(
        ffi::avcodec_send_packet(context, packet),
        "send audio packet",
    )?;
    while ffi::avcodec_receive_frame(context, frame) >= 0 {
        let processing_started = Instant::now();
        if output.is_none() {
            *output = Some(AudioOutput::open(
                (*frame).sample_rate,
                (*frame).ch_layout.nb_channels as u32,
                (*frame).format,
            )?);
        }
        let audio = output.as_mut().unwrap();
        let mut samples = vec![0u8; (*frame).nb_samples as usize * 2 * 2];
        let mut output_ptr = samples.as_mut_ptr();
        let count = ffi::swr_convert(
            audio.swr,
            &mut output_ptr,
            (*frame).nb_samples,
            (*frame).extended_data as *const *const u8,
            (*frame).nb_samples,
        );
        if count > 0 {
            apply_volume(&mut samples, count, snapshot);
            audio.write(&samples, count as u64)?;
            snapshot.lock().unwrap().audio_processing_ms =
                Some(processing_started.elapsed().as_secs_f64() * 1000.0);
        }
    }
    Ok(())
}

fn apply_volume(samples: &mut [u8], count: i32, snapshot: &Arc<Mutex<Snapshot>>) {
    let state = snapshot.lock().unwrap().clone();
    let gain: f32 = if state.muted {
        0.0
    } else {
        (state.volume / 100.0).clamp(0.0, 1.0) as f32
    };
    for sample in samples[..count as usize * 2 * 2].chunks_exact_mut(2) {
        let value = i16::from_ne_bytes([sample[0], sample[1]]);
        let scaled = (value as f32 * gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        sample.copy_from_slice(&scaled.to_ne_bytes());
    }
}

fn check(code: i32, operation: &str) -> Result<(), String> {
    if code < 0 {
        Err(format!("{operation} failed ({code})"))
    } else {
        Ok(())
    }
}
