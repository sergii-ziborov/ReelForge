//! Waveform peak extraction hooks for Capture / editor UI.

use crate::error::{IoError, Result};
use reelforge_core::{AudioClip, Time};
use serde::{Deserialize, Serialize};

/// One waveform bucket (min/max peak over a time window).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WaveformPeak {
    /// Window start seconds.
    pub t0: f64,
    /// Window end seconds.
    pub t1: f64,
    /// Minimum sample (after channel peak-hold).
    pub min: f32,
    /// Maximum sample (after channel peak-hold).
    pub max: f32,
}

/// Options for [`compute_waveform`].
#[derive(Debug, Clone)]
pub struct WaveformOptions {
    /// Number of peak buckets across the full duration (default 200).
    pub buckets: usize,
    /// Frames per read when sampling (default 2048).
    pub chunk_frames: usize,
}

impl Default for WaveformOptions {
    fn default() -> Self {
        Self {
            buckets: 200,
            chunk_frames: 2048,
        }
    }
}

impl WaveformOptions {
    /// Construct with bucket count.
    #[must_use]
    pub fn new(buckets: usize) -> Self {
        Self {
            buckets: buckets.max(1),
            chunk_frames: 2048,
        }
    }
}

/// Compute min/max peaks across `clip` for UI waveforms.
///
/// Channels are peak-held (max abs) into a mono envelope before bucketing.
///
/// # Errors
///
/// Sample read failures or empty duration.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn compute_waveform(clip: &dyn AudioClip, options: &WaveformOptions) -> Result<Vec<WaveformPeak>> {
    let duration = clip.duration();
    if !duration.is_positive() {
        return Err(IoError::message("waveform: clip duration must be > 0"));
    }
    let buckets = options.buckets.max(1);
    let fmt = clip.format();
    let total_frames = fmt.frames_for_duration(duration).max(1);
    let chunk = options.chunk_frames.max(64);

    let mut mins = vec![0.0_f32; buckets];
    let mut maxs = vec![0.0_f32; buckets];
    let mut seen = vec![false; buckets];

    let mut frame_i = 0_u64;
    while frame_i < total_frames {
        let remain = usize::try_from(total_frames - frame_i).unwrap_or(usize::MAX);
        let n = remain.min(chunk);
        let t = Time::from_secs(frame_i as f64 / f64::from(fmt.sample_rate));
        let buf = clip.samples_at(t, n).map_err(IoError::from)?;
        let ch = fmt.channels() as usize;
        let samples = buf.samples();
        let got = buf.frame_count();
        for f in 0..got {
            let base = f * ch;
            let mut peak = 0.0_f32;
            for c in 0..ch {
                peak = peak.max(samples[base + c].abs());
            }
            let lo = -peak;
            let hi = peak;
            let gi = frame_i + f as u64;
            let b = ((u128::from(gi) * u128::from(buckets as u64))
                / u128::from(total_frames)) as usize;
            let b = b.min(buckets - 1);
            if seen[b] {
                mins[b] = mins[b].min(lo);
                maxs[b] = maxs[b].max(hi);
            } else {
                mins[b] = lo;
                maxs[b] = hi;
                seen[b] = true;
            }
        }
        frame_i += got as u64;
        if got == 0 {
            break;
        }
    }

    let dur = duration.as_secs();
    let mut out = Vec::with_capacity(buckets);
    for i in 0..buckets {
        let t0 = dur * (i as f64) / buckets as f64;
        let t1 = dur * ((i + 1) as f64) / buckets as f64;
        let (min, max) = if seen[i] {
            (mins[i], maxs[i])
        } else {
            (0.0, 0.0)
        };
        out.push(WaveformPeak { t0, t1, min, max });
    }
    Ok(out)
}

/// Convenience with default options.
///
/// # Errors
///
/// Same as [`compute_waveform`].
pub fn compute_waveform_default(clip: &dyn AudioClip) -> Result<Vec<WaveformPeak>> {
    compute_waveform(clip, &WaveformOptions::default())
}

/// JSON-serialize peaks (pretty).
///
/// # Errors
///
/// Serde failure.
pub fn waveform_to_json(peaks: &[WaveformPeak]) -> Result<String> {
    serde_json::to_string_pretty(peaks).map_err(|e| IoError::message(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{AudioFormat, Duration, SilenceClip};
    use std::sync::Arc;

    #[test]
    fn silence_waveform_near_zero() {
        let clip = SilenceClip::new(AudioFormat::STEREO_48K, Duration::from_secs(0.1));
        let peaks = compute_waveform(&clip, &WaveformOptions::new(20)).unwrap();
        assert_eq!(peaks.len(), 20);
        assert!(peaks.iter().all(|p| p.max.abs() < 1e-5 && p.min.abs() < 1e-5));
    }

    #[test]
    fn json_roundtrip_shape() {
        let clip: Arc<dyn AudioClip> = Arc::new(SilenceClip::new(
            AudioFormat::STEREO_48K,
            Duration::from_secs(0.05),
        ));
        let peaks = compute_waveform_default(clip.as_ref()).unwrap();
        let json = waveform_to_json(&peaks).unwrap();
        assert!(json.contains("\"min\""));
    }
}
