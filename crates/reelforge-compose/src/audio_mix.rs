//! Multi-track audio mix (sum with optional per-track gains).

use crate::audio_read::{read_source, source_frames, tick_at};
use crate::{ComposeError, Result};
use reelforge_core::{
    AudioBuffer, AudioClip, AudioFormat, AudioMixMode, AudioTimeline, CoreError, Duration,
    MediaTime, Time,
};
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
        self.gain = if gain.is_finite() {
            gain.max(0.0)
        } else {
            gain
        };
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
/// [`Self::new`] retains legacy hard clipping to `[-1, 1]`.
/// Use [`Self::new_with_mode`] with [`AudioMixMode::Linear`] for internal buses.
/// Floating offsets and durations round once onto the format's sample clock.
/// Negative offsets provide preroll; reads only touch active source samples.
#[derive(Clone)]
pub struct MixAudio {
    tracks: Vec<MixTrack>,
    format: AudioFormat,
    duration: Duration,
    intervals: Vec<(i64, i64)>,
    mode: AudioMixMode,
}

impl MixAudio {
    /// Build a mix from tracks.
    ///
    /// # Errors
    ///
    /// Empty list or mismatched formats.
    pub fn new(tracks: Vec<MixTrack>) -> Result<Self> {
        Self::new_with_mode(tracks, AudioMixMode::HardClip)
    }

    /// Build a bus with explicit output clipping or linear float headroom.
    ///
    /// # Errors
    ///
    /// Empty or incompatible inputs, invalid gains or sample-clock intervals.
    pub fn new_with_mode(tracks: Vec<MixTrack>, mode: AudioMixMode) -> Result<Self> {
        if tracks.is_empty() {
            return Err(ComposeError::Message(
                "mix_audio requires at least one track".into(),
            ));
        }
        let format = tracks[0].clip.format();
        let mut last = 0_i64;
        let mut intervals = Vec::with_capacity(tracks.len());
        for t in &tracks {
            if t.clip.format() != format {
                return Err(ComposeError::Message(format!(
                    "all mix tracks must share format {format:?}, found {:?}",
                    t.clip.format()
                )));
            }
            if !t.gain.is_finite() || t.gain < 0.0 {
                return Err(
                    CoreError::invalid_audio("mix gain must be finite and nonnegative").into(),
                );
            }
            let start = tick_at(t.start, format)?;
            let end = start
                .checked_add(source_frames(t.clip.as_ref())?)
                .ok_or_else(|| CoreError::invalid_timing("mix sample clock overflow"))?;
            last = last.max(end);
            intervals.push((start, end));
        }
        Ok(Self {
            tracks,
            format,
            duration: format
                .duration_of_frames(u64::try_from(last).expect("nonnegative sample count")),
            intervals,
            mode,
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

    fn samples_at(&self, t: Time, frame_count: usize) -> reelforge_core::Result<AudioBuffer> {
        self.samples_at_media(
            MediaTime::new(tick_at(t, self.format)?, self.format.sample_rate)?,
            frame_count,
        )
    }

    fn samples_at_media(
        &self,
        t: MediaTime,
        frame_count: usize,
    ) -> reelforge_core::Result<AudioBuffer> {
        if frame_count == 0 {
            return AudioBuffer::silence(self.format, 0);
        }
        if t.ticks < 0 || t.timescale == 0 {
            return Err(CoreError::invalid_timing(
                "mix reads require a nonnegative valid clock",
            ));
        }
        let first = i64::try_from(AudioTimeline::from_format(self.format)?.index_at(t))
            .map_err(|_| CoreError::invalid_timing("mix read clock overflow"))?;
        let count = i64::try_from(frame_count)
            .map_err(|_| CoreError::invalid_audio("mix block too large"))?;
        let end = first
            .checked_add(count)
            .ok_or_else(|| CoreError::invalid_timing("mix read clock overflow"))?;
        let ch = usize::from(self.format.channels());
        let mut output = AudioBuffer::silence(self.format, frame_count)?;
        for (track, &(track_start, track_end)) in self.tracks.iter().zip(&self.intervals) {
            let a = first.max(track_start);
            let b = end.min(track_end);
            if a >= b {
                continue;
            }
            let skip = usize::try_from(a - first).expect("overlap inside requested block");
            let need = usize::try_from(b - a).expect("overlap fits requested block");
            let local = a
                .checked_sub(track_start)
                .ok_or_else(|| CoreError::invalid_timing("mix local clock overflow"))?;
            let buf = read_source(track.clip.as_ref(), self.format, local, need)?;
            for (out, sample) in output.samples_mut()[skip * ch..(skip + need) * ch]
                .iter_mut()
                .zip(buf.samples())
            {
                *out += sample * track.gain;
                if !out.is_finite() {
                    return Err(CoreError::invalid_audio("mix accumulation overflow"));
                }
            }
        }
        if self.mode == AudioMixMode::HardClip {
            for sample in output.samples_mut() {
                *sample = sample.clamp(-1.0, 1.0);
            }
        }
        Ok(output)
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

/// Mix tracks with an explicit bus output mode.
///
/// # Errors
///
/// Propagates [`MixAudio::new_with_mode`].
pub fn mix_audio_with_mode(
    tracks: Vec<MixTrack>,
    mode: AudioMixMode,
) -> Result<Arc<dyn AudioClip>> {
    Ok(Arc::new(MixAudio::new_with_mode(tracks, mode)?))
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
