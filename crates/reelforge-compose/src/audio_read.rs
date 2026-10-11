//! Shared checked PCM clock and bounded source reads.

use reelforge_core::{AudioBuffer, AudioClip, AudioFormat, CoreError, MediaTime, Time};

pub(crate) fn tick_at(t: Time, format: AudioFormat) -> reelforge_core::Result<i64> {
    AudioFormat::new(format.sample_rate, format.layout)?;
    let ticks = t.as_secs() * f64::from(format.sample_rate);
    // Keep the floating API's rounding at one boundary, then use integer clocks.
    if !ticks.is_finite()
        || !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&ticks)
    {
        return Err(CoreError::invalid_timing(
            "audio time exceeds the sample clock",
        ));
    }
    Ok(MediaTime::from_time(t, format.sample_rate)?.ticks)
}

pub(crate) fn source_frames(clip: &dyn AudioClip) -> reelforge_core::Result<i64> {
    let frames = tick_at(Time::from_secs(clip.duration().as_secs()), clip.format())?;
    if frames <= 0 {
        return Err(CoreError::invalid_audio(
            "audio source needs at least one sample frame",
        ));
    }
    Ok(frames)
}

pub(crate) fn read_source(
    clip: &dyn AudioClip,
    format: AudioFormat,
    first: i64,
    frames: usize,
) -> reelforge_core::Result<AudioBuffer> {
    let buffer = clip.samples_at_media(MediaTime::new(first, format.sample_rate)?, frames)?;
    if buffer.format() != format || buffer.frame_count() != frames {
        return Err(CoreError::invalid_audio(
            "audio source returned an incomplete or foreign-format block",
        ));
    }
    if buffer.samples().iter().any(|sample| !sample.is_finite()) {
        return Err(CoreError::invalid_audio(
            "audio source returned nonfinite PCM",
        ));
    }
    Ok(buffer)
}
