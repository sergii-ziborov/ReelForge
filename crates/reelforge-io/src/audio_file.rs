//! File-backed audio clips via the `FFmpeg` CLI.

use crate::error::{IoError, Result};
use crate::ffmpeg::{FfmpegTools, decode_pcm_f32le, default_pcm_format, probe_audio};
use crate::options::OpenAudioOptions;
use reelforge_core::{
    AudioBuffer, AudioClip, AudioFormat, CoreError, Duration, SampleLayout, Time,
};
use std::path::{Path, PathBuf};

/// Audio clip that holds decoded PCM in memory.
#[derive(Debug, Clone)]
pub struct AudioFileClip {
    path: PathBuf,
    format: AudioFormat,
    duration: Duration,
    buffer: AudioBuffer,
}

impl AudioFileClip {
    /// Open and fully decode an audio (or media) file to PCM.
    ///
    /// # Errors
    ///
    /// Returns tool, probe, or decode errors.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let options = OpenAudioOptions::new(path.as_ref().to_string_lossy());
        Self::open_with(&options)
    }

    /// Open with decode options.
    ///
    /// # Errors
    ///
    /// Returns tool, probe, or decode errors.
    pub fn open_with(options: &OpenAudioOptions) -> Result<Self> {
        let path = PathBuf::from(&options.path);
        if !path.is_file() {
            return Err(IoError::message(format!(
                "audio file not found: {}",
                path.display()
            )));
        }
        let tools = FfmpegTools::discover()?;
        let probe = probe_audio(&tools, &path)?;
        let format = default_pcm_format(options.sample_rate, options.stereo);
        let buffer = decode_pcm_f32le(&tools, &path, format)?;
        let duration = if buffer.duration().is_positive() {
            buffer.duration()
        } else {
            probe.duration
        };
        Ok(Self {
            path,
            format,
            duration,
            buffer,
        })
    }

    /// Source path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Full decoded buffer.
    #[must_use]
    pub fn buffer(&self) -> &AudioBuffer {
        &self.buffer
    }
}

impl AudioClip for AudioFileClip {
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
        if !self.contains(t) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }

        let ch = self.format.channels() as usize;
        let start_frame = usize::try_from(self.format.frames_for_duration(t.as_duration()))
            .map_err(|_| CoreError::invalid_audio("start frame index exceeds usize"))?;
        let total_frames = self.buffer.frame_count();
        if start_frame >= total_frames {
            return AudioBuffer::silence(self.format, frame_count);
        }

        let available = total_frames - start_frame;
        let take = frame_count.min(available);
        let start = start_frame * ch;
        let end = (start_frame + take) * ch;
        let mut samples = self.buffer.samples()[start..end].to_vec();
        if take < frame_count {
            samples.resize(frame_count * ch, 0.0);
        }
        AudioBuffer::from_interleaved(self.format, samples)
    }
}

/// Open an audio file (convenience wrapper).
///
/// # Errors
///
/// Propagates [`AudioFileClip::open_with`] errors.
pub fn open_audio(options: &OpenAudioOptions) -> Result<AudioFileClip> {
    AudioFileClip::open_with(options)
}

/// Infer layout helper for tests / callers.
#[must_use]
pub fn layout_for_channels(channels: u16) -> SampleLayout {
    if channels >= 2 {
        SampleLayout::Stereo
    } else {
        SampleLayout::Mono
    }
}
