//! Sequential audio concatenation.

use crate::audio_read::{read_source, source_frames, tick_at};
use crate::{ComposeError, Result};
use reelforge_core::{
    AudioBuffer, AudioClip, AudioFormat, AudioTimeline, CoreError, Duration, MediaTime, Time,
};
use std::sync::Arc;

/// Concatenate audio clips end-to-end.
///
/// All clips must share the same [`AudioFormat`]. Duration is the sum of inputs.
/// Input durations and floating reads round once to the nearest PCM frame.
/// Exact reads use the absolute sample clock. A block crosses every segment;
/// only its portion after the concatenation's end is padded with silence.
#[derive(Clone)]
pub struct ConcatAudio {
    clips: Vec<Arc<dyn AudioClip>>,
    ends: Vec<i64>,
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
        let mut total = 0_i64;

        for clip in &clips {
            if clip.format() != format {
                return Err(ComposeError::Message(format!(
                    "all audio clips must share format {format:?}, found {:?}",
                    clip.format()
                )));
            }
            total = total
                .checked_add(source_frames(clip.as_ref())?)
                .ok_or_else(|| CoreError::invalid_timing("concat sample clock overflow"))?;
            ends.push(total);
        }

        Ok(Self {
            clips,
            ends,
            format,
            duration: format
                .duration_of_frames(u64::try_from(total).expect("positive sample count")),
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
        let first = i64::try_from(AudioTimeline::from_format(self.format)?.index_at(t))
            .map_err(|_| CoreError::invalid_timing("concat read clock overflow"))?;
        if t.ticks < 0 || t.timescale == 0 || first >= *self.ends.last().expect("nonempty concat") {
            return Err(CoreError::TimeOutOfRange {
                time: t.to_time(),
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }
        let mut output = AudioBuffer::silence(self.format, frame_count)?;
        let ch = usize::from(self.format.channels());
        let mut written = 0;
        let mut cursor = first;
        let mut index = self.ends.partition_point(|end| *end <= first);
        while written < frame_count && index < self.clips.len() {
            let start = if index == 0 { 0 } else { self.ends[index - 1] };
            let available = usize::try_from(self.ends[index] - cursor).unwrap_or(usize::MAX);
            let need = (frame_count - written).min(available);
            let buffer = read_source(
                self.clips[index].as_ref(),
                self.format,
                cursor - start,
                need,
            )?;
            output.samples_mut()[written * ch..(written + need) * ch]
                .copy_from_slice(buffer.samples());
            written += need;
            cursor = self.ends[index];
            index += 1;
        }
        Ok(output)
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
