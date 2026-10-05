//! Playback speed / duration scale.

use reelforge_core::{
    AudioBuffer, AudioClip, AudioEffect, AudioFormat, CoreError, Duration, Frame, MediaTime,
    Result, Size, Time, VideoClip, VideoEffect, VideoSurface,
};
use std::sync::Arc;

/// Multiply playback speed by `factor` (`2.0` = twice as fast, half duration).
///
/// Applies to video (time remap) or audio (time remap of sample windows).
#[derive(Debug, Clone, Copy)]
pub struct Speed {
    /// Speed factor; must be finite and `> 0`.
    pub factor: f64,
}

impl Speed {
    /// Construct a speed effect.
    #[must_use]
    pub const fn new(factor: f64) -> Self {
        Self { factor }
    }
}

fn validate_factor(factor: f64) -> Result<()> {
    if factor.is_finite() && factor > 0.0 {
        Ok(())
    } else {
        Err(CoreError::invalid_timing(format!(
            "speed factor must be finite and > 0, got {factor}"
        )))
    }
}

impl VideoEffect for Speed {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        validate_factor(self.factor)?;
        Ok(Arc::new(SpeedVideo {
            inner: clip,
            factor: self.factor,
        }))
    }
}

impl AudioEffect for Speed {
    fn apply(&self, clip: Arc<dyn AudioClip>) -> Result<Arc<dyn AudioClip>> {
        validate_factor(self.factor)?;
        Ok(Arc::new(SpeedAudio {
            inner: clip,
            factor: self.factor,
        }))
    }
}

struct SpeedVideo {
    inner: Arc<dyn VideoClip>,
    factor: f64,
}

impl VideoClip for SpeedVideo {
    fn duration(&self) -> Duration {
        Duration::from_secs(self.inner.duration().as_secs() / self.factor)
    }

    fn size(&self) -> Size {
        self.inner.size()
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps().map(|f| f * self.factor)
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        if !self.contains(t) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration().as_secs())),
            });
        }
        let src_t = Time::from_secs(t.as_secs() * self.factor);
        // Clamp into source half-open range.
        let max_t = (self.inner.duration().as_secs() - f64::EPSILON).max(0.0);
        let src_t = Time::from_secs(src_t.as_secs().min(max_t));
        self.inner.frame_at(src_t)
    }

    fn surface_at(&self, t: Time) -> Result<VideoSurface> {
        if !self.contains(t) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration().as_secs())),
            });
        }
        let src_t = Time::from_secs(t.as_secs() * self.factor);
        let max_t = (self.inner.duration().as_secs() - f64::EPSILON).max(0.0);
        let src_t = Time::from_secs(src_t.as_secs().min(max_t));
        self.inner.surface_at(src_t)
    }
}

fn pcm_at(samples: &[f32], channels: usize, frames: usize, frame: usize, channel: usize) -> f32 {
    if channels == 0 || frame >= frames {
        return 0.0;
    }
    samples
        .get(frame * channels + channel)
        .copied()
        .unwrap_or(0.0)
}

struct SpeedAudio {
    inner: Arc<dyn AudioClip>,
    factor: f64,
}

impl AudioClip for SpeedAudio {
    fn duration(&self) -> Duration {
        Duration::from_secs(self.inner.duration().as_secs() / self.factor)
    }

    fn format(&self) -> AudioFormat {
        self.inner.format()
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn samples_at(&self, t: Time, frame_count: usize) -> Result<AudioBuffer> {
        if frame_count == 0 {
            return AudioBuffer::silence(self.format(), 0);
        }
        if !self.contains(t) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration().as_secs())),
            });
        }
        // Output sample (t*rate + i) reads source frame (t*rate + i)*factor.
        // The window starts on the floored source frame, so a later chunk keeps
        // the fractional phase instead of snapping to that frame.
        let fmt = self.format();
        let channels = usize::from(fmt.channels());
        let factor = self.factor;
        let rate_u = fmt.sample_rate.max(1);
        let rate = f64::from(rate_u);
        let origin = t.as_secs() * rate;
        let first_src = (origin * factor).max(0.0);
        let last_src = first_src + (frame_count.saturating_sub(1) as f64) * factor;
        let base = first_src.floor().max(0.0);
        let base_idx = base as usize;
        let last_needed = (last_src.floor() as usize).saturating_add(1);
        let need = last_needed
            .saturating_sub(base_idx)
            .saturating_add(1)
            .max(1);
        let src_secs = base / rate;
        let src = if src_secs < self.inner.duration().as_secs() {
            let base_i = i64::try_from(base_idx)
                .map_err(|_| CoreError::invalid_timing("speed source frame exceeds i64"))?;
            let mt = MediaTime::new(base_i, rate_u)?;
            self.inner.samples_at_media(mt, need)?
        } else {
            AudioBuffer::silence(fmt, need)?
        };
        let src_samples = src.samples();
        let src_frames_got = src_samples.len() / channels.max(1);
        let mut out = vec![0.0_f32; frame_count.saturating_mul(channels)];
        for i in 0..frame_count {
            let src_pos = ((origin + i as f64) * factor - base).max(0.0);
            let i0 = src_pos.floor() as usize;
            let frac = (src_pos - i0 as f64) as f32;
            let i1 = i0.saturating_add(1);
            for c in 0..channels {
                let a = pcm_at(src_samples, channels, src_frames_got, i0, c);
                let b = pcm_at(src_samples, channels, src_frames_got, i1, c);
                out[i * channels + c] = a + (b - a) * frac;
            }
        }
        AudioBuffer::from_interleaved(fmt, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn audio_speed_is_independent_of_chunk_size() {
        use reelforge_core::{AudioEffect, AudioFormat, SampleLayout};
        struct Ramp {
            frames: usize,
        }
        impl AudioClip for Ramp {
            fn duration(&self) -> Duration {
                Duration::from_secs(self.frames as f64 / 8.0)
            }
            fn format(&self) -> AudioFormat {
                AudioFormat::new(8, SampleLayout::Mono).unwrap()
            }
            fn samples_at(&self, t: Time, frame_count: usize) -> Result<AudioBuffer> {
                let start = (t.as_secs() * 8.0).round() as usize;
                let mut samples = Vec::with_capacity(frame_count);
                for i in 0..frame_count {
                    let idx = start + i;
                    let v = if idx < self.frames {
                        idx as f32 * 0.01
                    } else {
                        0.0
                    };
                    samples.push(v);
                }
                AudioBuffer::from_interleaved(self.format(), samples)
            }
        }
        let sped = AudioEffect::apply(&Speed::new(2.0), Arc::new(Ramp { frames: 32 })).unwrap();
        let whole = sped.samples_at(Time::ZERO, 4).unwrap();
        let a = sped.samples_at(Time::ZERO, 2).unwrap();
        let b = sped.samples_at(Time::from_secs(2.0 / 8.0), 2).unwrap();
        let mut chunked = a.samples().to_vec();
        chunked.extend_from_slice(b.samples());
        for (left, right) in whole.samples().iter().zip(chunked.iter()) {
            assert!((left - right).abs() < 1e-4, "{left} vs {right}");
        }
    }

    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn audio_speed_keeps_fractional_phase_across_chunks() {
        use reelforge_core::{AudioEffect, AudioFormat, SampleLayout};
        struct Ramp {
            frames: usize,
        }
        impl AudioClip for Ramp {
            fn duration(&self) -> Duration {
                Duration::from_secs(self.frames as f64 / 8.0)
            }
            fn format(&self) -> AudioFormat {
                AudioFormat::new(8, SampleLayout::Mono).unwrap()
            }
            fn samples_at(&self, t: Time, frame_count: usize) -> Result<AudioBuffer> {
                let start = (t.as_secs() * 8.0).round() as usize;
                let mut samples = Vec::with_capacity(frame_count);
                for i in 0..frame_count {
                    let idx = start + i;
                    let v = if idx < self.frames {
                        idx as f32 * 0.01
                    } else {
                        0.0
                    };
                    samples.push(v);
                }
                AudioBuffer::from_interleaved(self.format(), samples)
            }
        }
        let sped = AudioEffect::apply(&Speed::new(1.5), Arc::new(Ramp { frames: 32 })).unwrap();
        let whole = sped.samples_at(Time::ZERO, 4).unwrap();
        let a = sped.samples_at(Time::ZERO, 2).unwrap();
        let b = sped.samples_at(Time::from_secs(2.0 / 8.0), 2).unwrap();
        let mut chunked = a.samples().to_vec();
        chunked.extend_from_slice(b.samples());
        for (left, right) in whole.samples().iter().zip(chunked.iter()) {
            assert!((left - right).abs() < 1e-4, "{left} vs {right}");
        }
        let expected = [0.0_f32, 0.015, 0.03, 0.045];
        for (got, want) in whole.samples().iter().zip(expected) {
            assert!((got - want).abs() < 1e-4, "{got} vs {want}");
        }
    }

    #[test]
    fn speed_halves_duration() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::RED,
            Duration::from_secs(4.0),
        ));
        let out = VideoEffect::apply(&Speed::new(2.0), clip).unwrap();
        assert!((out.duration().as_secs() - 2.0).abs() < 1e-9);
        let _ = out.frame_at(Time::from_secs(1.0)).unwrap();
    }

    #[test]
    fn speed_forwards_native_surface() {
        use reelforge_core::{
            ColorInfo, MediaTime, PixelFormat, StreamTimeBase, SurfacePlane, VideoSurface,
        };
        struct YuvClip;
        impl VideoClip for YuvClip {
            fn duration(&self) -> Duration {
                Duration::from_secs(2.0)
            }
            fn size(&self) -> Size {
                Size::new(8, 4)
            }
            fn fps(&self) -> Option<f64> {
                Some(10.0)
            }
            fn frame_at(&self, t: Time) -> Result<Frame> {
                self.surface_at(t)?.to_rgb_frame()
            }
            fn surface_at(&self, t: Time) -> Result<VideoSurface> {
                if !self.contains(t) {
                    return Err(CoreError::TimeOutOfRange {
                        time: t,
                        range: (Time::ZERO, Time::from_secs(2.0)),
                    });
                }
                let y = SurfacePlane::new(8, 4, 8, vec![16_u8; 32]).unwrap();
                let u = SurfacePlane::new(4, 2, 4, vec![80_u8; 8]).unwrap();
                let v = SurfacePlane::new(4, 2, 4, vec![200_u8; 8]).unwrap();
                VideoSurface::from_planes(
                    PixelFormat::Yuv420p,
                    Size::new(8, 4),
                    vec![y, u, v],
                    MediaTime::zero(1_000),
                    None,
                    ColorInfo::default(),
                    StreamTimeBase::HZ_1K,
                )
            }
        }
        let out = VideoEffect::apply(&Speed::new(2.0), Arc::new(YuvClip)).unwrap();
        let s = out.surface_at(Time::from_secs(0.2)).unwrap();
        assert_eq!(s.format(), PixelFormat::Yuv420p);
        assert_eq!(s.planes().len(), 3);
    }
}
