//! Presentation-timestamp indexes for VFR / PTS-accurate frame mapping.

use crate::error::{IoError, Result};
use crate::ffmpeg::path::FfmpegTools;
use reelforge_core::MediaTime;
use std::path::Path;
use std::process::Command;

/// Ordered frame presentation times for a video stream.
///
/// Built from `ffprobe` packet/`pts_time` lists or constructed in tests.
/// Maps media time → frame ordinal via last PTS ≤ query (binary search).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameTimingIndex {
    /// Presentation timestamps as ticks at [`Self::timescale`], strictly non-decreasing.
    pts_ticks: Vec<i64>,
    /// Ticks per second (typically `1_000_000` for microsecond `pts_time` import).
    timescale: u32,
}

impl FrameTimingIndex {
    /// Empty index (treat as CFR fallback).
    #[must_use]
    pub fn empty(timescale: u32) -> Self {
        Self {
            pts_ticks: Vec::new(),
            timescale: timescale.max(1),
        }
    }

    /// Build from sorted presentation times in seconds.
    ///
    /// Drops non-finite values; sorts and de-duplicates identical ticks.
    ///
    /// # Errors
    ///
    /// Zero timescale.
    pub fn from_pts_secs(
        times_secs: impl IntoIterator<Item = f64>,
        timescale: u32,
    ) -> Result<Self> {
        if timescale == 0 {
            return Err(IoError::message("FrameTimingIndex timescale must be > 0"));
        }
        let mut pts_ticks = Vec::new();
        for s in times_secs {
            if !s.is_finite() || s < 0.0 {
                continue;
            }
            let mt =
                MediaTime::from_secs(s, timescale).map_err(|e| IoError::message(e.to_string()))?;
            pts_ticks.push(mt.ticks);
        }
        pts_ticks.sort_unstable();
        pts_ticks.dedup();
        Ok(Self {
            pts_ticks,
            timescale,
        })
    }

    /// Build from raw PTS values and `FFmpeg` `time_base = num/den`.
    ///
    /// # Errors
    ///
    /// Invalid time base.
    pub fn from_pts_raw(
        pts_values: impl IntoIterator<Item = i64>,
        time_base_num: u32,
        time_base_den: u32,
    ) -> Result<Self> {
        if time_base_den == 0 || time_base_num == 0 {
            return Err(IoError::message("invalid time_base for FrameTimingIndex"));
        }
        let mut pts_ticks = Vec::new();
        for pts in pts_values {
            if pts < 0 {
                continue;
            }
            let mt = MediaTime::from_pts(pts, time_base_num, time_base_den)
                .map_err(|e| IoError::message(e.to_string()))?;
            pts_ticks.push(mt.ticks);
        }
        pts_ticks.sort_unstable();
        pts_ticks.dedup();
        Ok(Self {
            pts_ticks,
            timescale: time_base_den,
        })
    }

    /// Number of indexed frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pts_ticks.len()
    }

    /// Whether no PTS samples were recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pts_ticks.is_empty()
    }

    /// Index timescale.
    #[must_use]
    pub fn timescale(&self) -> u32 {
        self.timescale
    }

    /// PTS at frame ordinal, when in range.
    #[must_use]
    pub fn pts_at(&self, index: u64) -> Option<MediaTime> {
        let i = usize::try_from(index).ok()?;
        let ticks = *self.pts_ticks.get(i)?;
        MediaTime::new(ticks, self.timescale).ok()
    }

    /// Frame duration as PTS delta to the next sample, when both exist.
    #[must_use]
    pub fn duration_at(&self, index: u64) -> Option<MediaTime> {
        let a = self.pts_at(index)?;
        let b = self.pts_at(index.checked_add(1)?)?;
        let dt = b.ticks.saturating_sub(a.ticks);
        if dt <= 0 {
            return None;
        }
        MediaTime::new(dt, self.timescale).ok()
    }

    /// Frame ordinal for media time `t` (last frame with PTS ≤ t; 0 if before first).
    #[must_use]
    pub fn frame_index_at(&self, t: MediaTime) -> u64 {
        if self.pts_ticks.is_empty() {
            return 0;
        }
        let query = t.rebase(self.timescale).map_or_else(
            |_| {
                #[allow(clippy::cast_possible_truncation)]
                {
                    (t.as_secs() * f64::from(self.timescale)).round() as i64
                }
            },
            |m| m.ticks,
        );
        // partition_point: first element > query → last ≤ query is idx-1
        let pos = self.pts_ticks.partition_point(|&p| p <= query);
        if pos == 0 {
            0
        } else {
            u64::try_from(pos - 1).unwrap_or(0)
        }
    }

    /// Half-open frame index range `[start, end)` covering media `[start_t, end_t)`.
    ///
    /// `end` is the first frame with PTS ≥ `end_t` (or `len` if past the end).
    #[must_use]
    pub fn frame_range(&self, start_t: MediaTime, end_t: MediaTime) -> (u64, u64) {
        if self.pts_ticks.is_empty() {
            return (0, 0);
        }
        let start = self.frame_index_at(start_t);
        let end_q = end_t.rebase(self.timescale).map_or(i64::MAX, |m| m.ticks);
        let end_pos = self.pts_ticks.partition_point(|&p| p < end_q);
        let end = u64::try_from(end_pos).unwrap_or(u64::MAX);
        if end <= start {
            (start, start)
        } else {
            (start, end)
        }
    }
}

/// Probe packet `pts_time` list for the primary video stream.
///
/// Uses `ffprobe` CSV of `packet=pts_time`. Optional `max_packets` caps work for
/// long files (0 = unlimited).
///
/// # Errors
///
/// Process / parse failures.
pub fn probe_frame_timing(
    tools: &FfmpegTools,
    path: &Path,
    max_packets: usize,
) -> Result<FrameTimingIndex> {
    let output = Command::new(&tools.ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .map_err(|e| IoError::process(format!("ffprobe pts spawn failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IoError::process(format!(
            "ffprobe pts exited {}: {stderr}",
            output.status
        )));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut secs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line == "N/A" {
            continue;
        }
        if let Ok(v) = line.parse::<f64>() {
            secs.push(v);
            if max_packets > 0 && secs.len() >= max_packets {
                break;
            }
        }
    }
    // Microsecond timescale for float pts_time import.
    FrameTimingIndex::from_pts_secs(secs, 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_vfr_times_to_ordinals() {
        // Irregular spacing: 0, 0.04, 0.10, 0.11, 0.20
        let idx = FrameTimingIndex::from_pts_secs([0.0, 0.04, 0.10, 0.11, 0.20], 1_000).unwrap();
        assert_eq!(idx.len(), 5);
        let t = |s: f64| MediaTime::from_secs(s, 1_000).unwrap();
        assert_eq!(idx.frame_index_at(t(0.0)), 0);
        assert_eq!(idx.frame_index_at(t(0.039)), 0);
        assert_eq!(idx.frame_index_at(t(0.04)), 1);
        assert_eq!(idx.frame_index_at(t(0.105)), 2);
        assert_eq!(idx.frame_index_at(t(0.11)), 3);
        assert_eq!(idx.frame_index_at(t(0.5)), 4);
        assert_eq!(idx.frame_range(t(0.04), t(0.11)), (1, 3));
        assert!((idx.pts_at(2).unwrap().as_secs() - 0.10).abs() < 1e-9);
        assert!((idx.duration_at(1).unwrap().as_secs() - 0.06).abs() < 1e-9);
        assert!(idx.duration_at(4).is_none());
    }

    #[test]
    fn raw_pts_time_base() {
        // time_base 1/30, pts 0,1,2 → 0, 1/30, 2/30 s
        let idx = FrameTimingIndex::from_pts_raw([0, 1, 2], 1, 30).unwrap();
        assert_eq!(idx.len(), 3);
        let t = MediaTime::from_pts(1, 1, 30).unwrap();
        assert_eq!(idx.frame_index_at(t), 1);
    }
}
