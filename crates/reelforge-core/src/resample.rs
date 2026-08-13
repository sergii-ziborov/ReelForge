//! In-process linear PCM resampling (rate only; layout is unchanged).

use crate::audio::{AudioBuffer, AudioFormat};
use crate::error::{CoreError, Result};

/// Resample `src` to `target_rate` with per-channel linear interpolation.
///
/// Same rate returns a clone. Empty buffers stay empty.
///
/// # Errors
///
/// Zero target rate or allocation overflow.
pub fn resample_linear(src: &AudioBuffer, target_rate: u32) -> Result<AudioBuffer> {
    if target_rate == 0 {
        return Err(CoreError::invalid_audio("target sample_rate must be > 0"));
    }
    let src_fmt = src.format();
    if src_fmt.sample_rate == target_rate {
        return Ok(src.clone());
    }
    let in_frames = src.frame_count();
    if in_frames == 0 {
        let fmt = AudioFormat::new(target_rate, src_fmt.layout)?;
        return AudioBuffer::silence(fmt, 0);
    }
    let ch = src_fmt.channels() as usize;
    let out_frames = scale_frame_count(in_frames, src_fmt.sample_rate, target_rate)?;
    if out_frames == 0 {
        let fmt = AudioFormat::new(target_rate, src_fmt.layout)?;
        return AudioBuffer::silence(fmt, 0);
    }
    let in_rate = f64::from(src_fmt.sample_rate);
    let out_rate = f64::from(target_rate);
    let samples_in = src.samples();
    let last = in_frames - 1;
    let mut out = vec![0.0_f32; out_frames.saturating_mul(ch)];
    for (i, frame) in out.chunks_exact_mut(ch).enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let pos = i as f64 * in_rate / out_rate;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i0 = pos.floor() as usize;
        let i0 = i0.min(last);
        let i1 = (i0 + 1).min(last);
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let frac = (pos - i0 as f64) as f32;
        let frac = frac.clamp(0.0, 1.0);
        let a = i0 * ch;
        let b = i1 * ch;
        for c in 0..ch {
            let s0 = samples_in[a + c];
            let s1 = samples_in[b + c];
            frame[c] = s0 + (s1 - s0) * frac;
        }
    }
    let fmt = AudioFormat::new(target_rate, src_fmt.layout)?;
    AudioBuffer::from_interleaved(fmt, out)
}

fn scale_frame_count(in_frames: usize, in_rate: u32, out_rate: u32) -> Result<usize> {
    let n = u128::try_from(in_frames).unwrap_or(u128::MAX);
    let scaled = n.saturating_mul(u128::from(out_rate)) / u128::from(in_rate.max(1));
    usize::try_from(scaled)
        .map_err(|_| CoreError::invalid_audio("resampled length overflows usize"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleLayout;

    #[test]
    fn identity_rate_is_clone() {
        let fmt = AudioFormat::new(8, SampleLayout::Mono).unwrap();
        let src = AudioBuffer::from_interleaved(fmt, vec![0.0, 0.5, 1.0, 0.5]).unwrap();
        let out = resample_linear(&src, 8).unwrap();
        assert_eq!(out.samples(), src.samples());
    }

    #[test]
    fn upsample_doubles_length_and_hits_endpoints() {
        let fmt = AudioFormat::new(2, SampleLayout::Mono).unwrap();
        let src = AudioBuffer::from_interleaved(fmt, vec![0.0, 1.0]).unwrap();
        let out = resample_linear(&src, 4).unwrap();
        assert_eq!(out.frame_count(), 4);
        assert!((out.samples()[0] - 0.0).abs() < 1e-6);
        assert!((out.samples()[3] - 1.0).abs() < 1e-6);
        assert!(out.samples()[1] > 0.0 && out.samples()[1] < 1.0);
    }

    #[test]
    fn stereo_keeps_channels() {
        let fmt = AudioFormat::STEREO_48K;
        let src = AudioBuffer::from_interleaved(fmt, vec![0.0, 1.0, 0.0, 1.0]).unwrap();
        let out = resample_linear(&src, 24_000).unwrap();
        assert_eq!(out.format().layout, fmt.layout);
        assert_eq!(out.format().channels(), 2);
        assert_eq!(out.frame_count(), 1);
    }
}
