use super::{
    super::{ffi, state::Snapshot},
    output::{AudioOutput, OUTPUT_RATE},
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
        let input_rate = (*frame).sample_rate.max(1) as i64;
        let input_samples = (*frame).nb_samples.max(0) as i64;
        let delayed_samples = ffi::swr_get_delay(audio.swr, input_rate);
        let output_capacity = ffi::av_rescale_rnd(
            delayed_samples.saturating_add(input_samples),
            OUTPUT_RATE,
            input_rate,
            ffi::AVRounding_AV_ROUND_UP,
        )
        .max(1) as usize;
        let mut samples = vec![0u8; output_capacity * 2 * 2];
        let mut output_ptr = samples.as_mut_ptr();
        let count = ffi::swr_convert(
            audio.swr,
            &mut output_ptr,
            output_capacity as i32,
            (*frame).extended_data as *const *const u8,
            (*frame).nb_samples,
        );
        if count > 0 {
            // `swr_convert` is allowed to produce fewer samples than the
            // capacity requested above. Keep only those bytes before putting
            // this allocation on the output queue: retaining the spare
            // capacity in every queued chunk needlessly inflates the audio
            // footprint, especially while the resampler is draining.
            let output_bytes = usize::try_from(count)
                .ok()
                .and_then(|frames| frames.checked_mul(2 * 2))
                .ok_or("resampled audio buffer size overflow")?;
            samples.truncate(output_bytes);
            // `truncate` changes only the visible length; reduce the backing
            // allocation as well before transferring ownership to the queue.
            samples.shrink_to(output_bytes);
            apply_volume(&mut samples, snapshot);
            // Transfer the resampler allocation to the queue. The previous
            // slice API cloned PCM here, temporarily keeping two full copies
            // of every decoded audio chunk alive.
            audio.write(samples, count as u64)?;
            snapshot.lock().unwrap().audio_processing_ms =
                Some(processing_started.elapsed().as_secs_f64() * 1000.0);
        }
    }
    Ok(())
}

fn apply_volume(samples: &mut [u8], snapshot: &Arc<Mutex<Snapshot>>) {
    // Volume changes are the only fields needed here. Cloning `Snapshot`
    // copied all media metadata once per decoded audio frame.
    let (muted, volume) = {
        let state = snapshot.lock().unwrap();
        (state.muted, state.volume)
    };
    let gain: f32 = if muted {
        0.0
    } else {
        (volume / 100.0).clamp(0.0, 1.0) as f32
    };
    for sample in samples.chunks_exact_mut(2) {
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
