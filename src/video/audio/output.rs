use super::super::ffi;

pub(crate) struct AudioOutput {
    pub(super) pcm: *mut ffi::snd_pcm_t,
    pub(super) swr: *mut ffi::SwrContext,
    recoveries: u32,
}

const OUTPUT_RATE: u32 = 48_000;
const OUTPUT_CHANNELS: u32 = 2;

impl AudioOutput {
    pub(super) unsafe fn open(
        rate: i32,
        channels: u32,
        sample_format: i32,
    ) -> Result<Self, String> {
        let mut pcm = std::ptr::null_mut();
        let name = std::ffi::CString::new("default").unwrap();
        check(
            ffi::snd_pcm_open(&mut pcm, name.as_ptr(), 0, 0),
            "open ALSA output",
        )?;
        check(
            ffi::snd_pcm_set_params(
                pcm,
                ffi::_snd_pcm_format_SND_PCM_FORMAT_S16_LE,
                ffi::_snd_pcm_access_SND_PCM_ACCESS_RW_INTERLEAVED,
                OUTPUT_CHANNELS,
                OUTPUT_RATE,
                1,
                500_000,
            ),
            "configure ALSA output",
        )?;
        let swr = super::resampler::create(sample_format, rate, channels)?;
        eprintln!(
            "veyra audio: ALSA default, input {} Hz/{} channels/format {}, output {} Hz/{} channels S16_LE",
            rate, channels, sample_format, OUTPUT_RATE, OUTPUT_CHANNELS
        );
        Ok(Self {
            pcm,
            swr,
            recoveries: 0,
        })
    }

    pub(super) unsafe fn write(&mut self, samples: &[u8], frames: u64) -> Result<(), String> {
        let bytes_per_frame = (OUTPUT_CHANNELS as usize) * 2;
        let mut offset = 0u64;
        while offset < frames {
            let pointer = samples
                .as_ptr()
                .add(offset as usize * bytes_per_frame)
                .cast();
            let written = ffi::snd_pcm_writei(self.pcm, pointer, frames - offset);
            if written < 0 {
                self.recoveries = self.recoveries.saturating_add(1);
                let recovered = ffi::snd_pcm_recover(self.pcm, written as i32, 1);
                if self.recoveries <= 3 {
                    eprintln!(
                        "veyra audio: snd_pcm_writei failed ({written}), recovery {} returned {recovered}",
                        self.recoveries
                    );
                }
                if recovered < 0 {
                    return Err(format!(
                        "audio output write failed ({written}), recovery failed ({recovered})"
                    ));
                }
                continue;
            }
            if written == 0 {
                return Err("audio output write made no progress".into());
            }
            offset += written as u64;
        }
        Ok(())
    }
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        unsafe {
            let mut swr = self.swr;
            ffi::swr_free(&mut swr);
            ffi::snd_pcm_drop(self.pcm);
            ffi::snd_pcm_close(self.pcm);
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
