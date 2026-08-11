//! Sequential audio concatenation.

use crate::timeline::map_concat_time;
use crate::{ComposeError, Result};
use reelforge_core::{AudioBuffer, AudioClip, AudioFormat, Duration, Time};
use std::sync::Arc;

/// Concatenate audio clips end-to-end.
///
/// All clips must share the same [`AudioFormat`]. Duration is the sum of inputs.
#[derive(Clone)]
pub struct ConcatAudio {
    clips: Vec<Arc<dyn AudioClip>>,
    ends: Vec<Duration>,
    format: AudioFormat,
    duration: Duration,
}

impl ConcatAudio {
    /// Build a concatenation of `clips` in order.
    ///
    /// # Errors
    ///
    /// Returns [`ComposeError`] when the list is empty or formats differ.
    pub fn new(clips: Vec<Arc<dyn AudioClip>>) -> Result<Self> {
        if clips.is_empty() {
            return Err(ComposeError::Message(
                "concatenate_audio requires at least one clip".into(),
            ));
        }
        let format = clips[0].format();
        let mut ends = Vec::with_capacity(clips.len());
        let mut total = Duration::ZERO;

        for clip in &clips {
            if clip.format() != format {
                return Err(ComposeError::Message(format!(
                    "all audio clips must share format {format:?}, found {:?}",
                    clip.format()
                )));
            }
            if !clip.duration().is_positive() {
                return Err(ComposeError::Message(
                    "each audio clip must have positive duration".into(),
                ));
            }
            total += clip.duration();
            ends.push(total);
        }

        Ok(Self {
            clips,
            ends,
            format,
            duration: total,
        })
    }
}

impl AudioClip for ConcatAudio {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn format(&self) -> AudioFormat {
        self.format
    }

    fn samples_at(&self, t: Time, frame_count: usize) -> reelforge_core::Result<AudioBuffer> {
        if frame_count == 0 {
            return AudioBuffer::silence(self.format, 0);
        }
        let (i, local) = map_concat_time(&self.ends, self.duration, t)?;
        // Simple path: pull from the clip that owns `t`. Spans across boundaries
        // zero-fill the remainder; boundary-aware mix can improve later.
        let mut buf = self.clips[i].samples_at(local, frame_count)?;
        let ch = self.format.channels() as usize;
        let got = buf.frame_count();
        if got < frame_count {
            let mut samples = buf.samples().to_vec();
            samples.resize(frame_count * ch, 0.0);
            buf = AudioBuffer::from_interleaved(self.format, samples)?;
        }
        Ok(buf)
    }
}

/// Concatenate audio clips; returns a trait object.
///
/// # Errors
///
/// Propagates [`ConcatAudio::new`] errors.
pub fn concatenate_audio(clips: Vec<Arc<dyn AudioClip>>) -> Result<Arc<dyn AudioClip>> {
    Ok(Arc::new(ConcatAudio::new(clips)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{AudioFormat, SilenceClip};

    #[test]
    fn concat_silence() {
        let a: Arc<dyn AudioClip> = Arc::new(SilenceClip::new(
            AudioFormat::STEREO_48K,
            Duration::from_secs(0.5),
        ));
        let b: Arc<dyn AudioClip> = Arc::new(SilenceClip::new(
            AudioFormat::STEREO_48K,
            Duration::from_secs(0.5),
        ));
        let cat = ConcatAudio::new(vec![a, b]).unwrap();
        assert!((cat.duration().as_secs() - 1.0).abs() < 1e-9);
        let buf = cat.samples_at(Time::from_secs(0.75), 16).unwrap();
        assert_eq!(buf.frame_count(), 16);
    }
}
