//! File-backed audio clips via the `FFmpeg` CLI.

use crate::error::{IoError, Result};
use crate::ffmpeg::{FfmpegTools, decode_pcm_f32le, default_pcm_format, probe_audio};
use crate::options::OpenAudioOptions;
use reelforge_core::{
    AudioBuffer, AudioClip, AudioFormat, AudioTimeline, CoreError, Duration, MediaTime,
    SampleLayout, Time,
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
        let format = resolve_open_format(options, probe.channels)
            .map_err(|e| IoError::message(e.to_string()))?;
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

    /// Sample-accurate timeline at this clip's rate.
    #[must_use]
    pub fn timeline(&self) -> AudioTimeline {
        AudioTimeline::from_format(self.format).unwrap_or(AudioTimeline {
            sample_rate: 1,
            timescale: 1,
        })
    }

    fn read_from_index(
        &self,
        start_frame: usize,
        frame_count: usize,
    ) -> reelforge_core::Result<AudioBuffer> {
        if frame_count == 0 {
            return AudioBuffer::silence(self.format, 0);
        }
        let ch = self.format.channels() as usize;
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

impl AudioClip for AudioFileClip {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn format(&self) -> AudioFormat {
        self.format
    }

    fn samples_at(&self, t: Time, frame_count: usize) -> reelforge_core::Result<AudioBuffer> {
        let rate = self.format.sample_rate.max(1);
        let mt = MediaTime::from_time(t, rate).unwrap_or_else(|_| MediaTime::zero(rate));
        self.samples_at_media(mt, frame_count)
    }

    fn samples_at_media(
        &self,
        t: MediaTime,
        frame_count: usize,
    ) -> reelforge_core::Result<AudioBuffer> {
        if frame_count == 0 {
            return AudioBuffer::silence(self.format, 0);
        }
        let t_float = t.to_time();
        if !self.contains(t_float) {
            return Err(CoreError::TimeOutOfRange {
                time: t_float,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }
        let start = usize::try_from(self.timeline().index_at(t))
            .map_err(|_| CoreError::invalid_audio("start frame index exceeds usize"))?;
        self.read_from_index(start, frame_count)
    }

    fn audio_timeline(&self) -> Option<AudioTimeline> {
        Some(self.timeline())
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

/// Infer a named layout from a channel count.
#[must_use]
pub fn layout_for_channels(channels: u16) -> SampleLayout {
    SampleLayout::from_channels(channels)
}

fn resolve_open_format(
    options: &OpenAudioOptions,
    probed: Option<u16>,
) -> reelforge_core::Result<AudioFormat> {
    if let Some(layout) = options.layout {
        return AudioFormat::new(options.sample_rate.max(1), layout);
    }
    if options.native_layout {
        return AudioFormat::new(
            options.sample_rate.max(1),
            SampleLayout::from_channels(probed.unwrap_or(2).max(1)),
        );
    }
    Ok(default_pcm_format(
        options.sample_rate.max(1),
        options.stereo,
    ))
}
