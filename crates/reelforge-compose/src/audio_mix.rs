//! Multi-track audio mix (sum with optional per-track gains).

use crate::{ComposeError, Result};
use reelforge_core::{AudioBuffer, AudioClip, AudioFormat, Duration, Time};
use std::sync::Arc;

/// One input bus in a mix.
#[derive(Clone)]
pub struct MixTrack {
    /// Source clip.
    pub clip: Arc<dyn AudioClip>,
    /// Linear gain (`1.0` = unity).
    pub gain: f32,
    /// Start time of this track on the mix timeline.
    pub start: Time,
}

impl MixTrack {
    /// Unity-gain track starting at `t = 0`.
    #[must_use]
    pub fn new(clip: Arc<dyn AudioClip>) -> Self {
        Self {
            clip,
            gain: 1.0,
            start: Time::ZERO,
        }
    }

    /// Set gain.
    #[must_use]
    pub fn with_gain(mut self, gain: f32) -> Self {
        self.gain = gain.max(0.0);
        self
    }

    /// Set start offset on the mix timeline.
    #[must_use]
    pub fn with_start(mut self, start: Time) -> Self {
        self.start = start;
        self
    }
}

/// Sum multiple audio tracks (same [`AudioFormat`]) into one bus.
///
/// Duration is the max of `start + clip.duration` across tracks.
/// Samples are summed and soft-clamped to `[-1, 1]`.
#[derive(Clone)]
pub struct MixAudio {
    tracks: Vec<MixTrack>,
    format: AudioFormat,
    duration: Duration,
}

impl MixAudio {
    /// Build a mix from tracks.
    ///
    /// # Errors
    ///
    /// Empty list or mismatched formats.
    pub fn new(tracks: Vec<MixTrack>) -> Result<Self> {
        if tracks.is_empty() {
            return Err(ComposeError::Message(
                "mix_audio requires at least one track".into(),
            ));
        }
        let format = tracks[0].clip.format();
        let mut duration = Duration::ZERO;
        for t in &tracks {
            if t.clip.format() != format {
                return Err(ComposeError::Message(format!(
                    "all mix tracks must share format {format:?}, found {:?}",
                    t.clip.format()
                )));
            }
            if !t.clip.duration().is_positive() {
                return Err(ComposeError::Message(
                    "each mix track must have positive duration".into(),
                ));
            }
            let end = Duration::from_secs(t.start.as_secs() + t.clip.duration().as_secs());
            if end.as_secs() > duration.as_secs() {
                duration = end;
            }
        }
        Ok(Self {
            tracks,
            format,
            duration,
        })
    }

    /// Tracks.
    #[must_use]
    pub fn tracks(&self) -> &[MixTrack] {
        &self.tracks
    }
}

impl AudioClip for MixAudio {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn format(&self) -> AudioFormat {
        self.format
    }

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn samples_at(&self, t: Time, frame_count: usize) -> reelforge_core::Result<AudioBuffer> {
        if frame_count == 0 {
            return AudioBuffer::silence(self.format, 0);
        }
        let ch = self.format.channels() as usize;
        let mut acc = vec![0.0_f32; frame_count * ch];
        let t0 = t.as_secs();
        let dt = 1.0 / f64::from(self.format.sample_rate);

        for track in &self.tracks {
            let start = track.start.as_secs();
            let end = start + track.clip.duration().as_secs();
            // Overlap of [t0, t0 + frame_count*dt) with [start, end)
            let mix_end = t0 + frame_count as f64 * dt;
            if mix_end <= start || t0 >= end {
                continue;
            }
            let local_t0 = (t0 - start).max(0.0);
            let skip_frames = if t0 < start {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    ((start - t0) / dt).ceil() as usize
                }
            } else {
                0
            };
            if skip_frames >= frame_count {
                continue;
            }
            let need = frame_count - skip_frames;
            let local_start = if t0 < start { 0.0 } else { local_t0 };
            let buf = track.clip.samples_at(Time::from_secs(local_start), need)?;
            let got = buf.frame_count().min(need);
            let samples = buf.samples();
            let g = track.gain;
            for f in 0..got {
                let di = (skip_frames + f) * ch;
                let si = f * ch;
                for c in 0..ch {
                    acc[di + c] += samples[si + c] * g;
                }
            }
        }

        for s in &mut acc {
            *s = s.clamp(-1.0, 1.0);
        }
        AudioBuffer::from_interleaved(self.format, acc)
    }
}

/// Mix tracks into a trait object.
///
/// # Errors
///
/// Propagates [`MixAudio::new`].
pub fn mix_audio(tracks: Vec<MixTrack>) -> Result<Arc<dyn AudioClip>> {
    Ok(Arc::new(MixAudio::new(tracks)?))
}

/// Convenience: mix equal-gain clips starting at zero.
///
/// # Errors
///
/// Empty or format mismatch.
pub fn mix_audio_clips(clips: Vec<Arc<dyn AudioClip>>) -> Result<Arc<dyn AudioClip>> {
    let tracks = clips.into_iter().map(MixTrack::new).collect();
    mix_audio(tracks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{AudioFormat, SilenceClip};

    #[test]
    fn mix_two_silence() {
        let a: Arc<dyn AudioClip> = Arc::new(SilenceClip::new(
            AudioFormat::STEREO_48K,
            Duration::from_secs(0.5),
        ));
        let b: Arc<dyn AudioClip> = Arc::new(SilenceClip::new(
            AudioFormat::STEREO_48K,
            Duration::from_secs(1.0),
        ));
        let m = MixAudio::new(vec![
            MixTrack::new(a),
            MixTrack::new(b).with_start(Time::from_secs(0.25)),
        ])
        .unwrap();
        assert!((m.duration().as_secs() - 1.25).abs() < 1e-6);
        let buf = m.samples_at(Time::ZERO, 32).unwrap();
        assert_eq!(buf.frame_count(), 32);
    }
}
