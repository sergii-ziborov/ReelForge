//! Declarative filter operations compiled to an `ffmpeg` filter string.

use serde::{Deserialize, Serialize};

/// One video transform expressible as an `FFmpeg` filter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FilterOp {
    /// Trim to `[start, start+duration)` seconds.
    Trim {
        /// Start time in seconds.
        start: f64,
        /// Length in seconds.
        duration: f64,
    },
    /// Crop `w:h:x:y`.
    Crop {
        /// Width.
        w: u32,
        /// Height.
        h: u32,
        /// X offset.
        x: u32,
        /// Y offset.
        y: u32,
    },
    /// Scale to width/height (`-1` keeps aspect if one side is 0 — use explicit sizes here).
    Scale {
        /// Output width.
        w: u32,
        /// Output height.
        h: u32,
    },
    /// Horizontal flip.
    HFlip,
    /// Vertical flip.
    VFlip,
    /// Rotate 90° clockwise (transpose).
    TransposeCw,
    /// Force even dimensions via crop.
    EvenDims,
    /// Fade in over seconds.
    FadeIn {
        /// Fade length.
        duration: f64,
    },
    /// Fade out over seconds at end (needs total duration).
    FadeOut {
        /// Fade length.
        duration: f64,
        /// Total media duration for start offset.
        total: f64,
    },
    /// Desaturate to grayscale (`hue=s=0`).
    BlackAndWhite,
}

/// Ordered filter chain for a single input → single output.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FilterGraph {
    /// Operations in order.
    pub ops: Vec<FilterOp>,
}

impl FilterGraph {
    /// Empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }

    /// Append an operation.
    #[must_use]
    pub fn then(mut self, op: FilterOp) -> Self {
        self.ops.push(op);
        self
    }

    /// Whether the graph has any ops.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Compile to an `FFmpeg` `-vf` filter string.
    ///
    /// # Errors
    ///
    /// Returns a message when the graph is empty.
    pub fn to_vf(&self) -> Result<String, String> {
        if self.ops.is_empty() {
            return Err("filter graph is empty".into());
        }
        let parts: Vec<String> = self.ops.iter().map(FilterOp::to_filter).collect();
        Ok(parts.join(","))
    }

    /// Audio filter matching video [`FilterOp::Trim`] ops (`atrim` + `asetpts`).
    ///
    /// `None` when the graph does not change the timeline — then `-c:a copy`
    /// can keep the source bitstream.
    #[must_use]
    pub fn to_af(&self) -> Option<String> {
        let parts: Vec<String> = self.ops.iter().filter_map(FilterOp::to_afilter).collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(","))
        }
    }
}

impl FilterOp {
    fn to_filter(&self) -> String {
        match self {
            Self::Trim { start, duration } => {
                format!("trim=start={start}:duration={duration},setpts=PTS-STARTPTS")
            }
            Self::Crop { w, h, x, y } => format!("crop={w}:{h}:{x}:{y}"),
            Self::Scale { w, h } => format!("scale={w}:{h}"),
            Self::HFlip => "hflip".into(),
            Self::VFlip => "vflip".into(),
            Self::TransposeCw => "transpose=1".into(),
            Self::EvenDims => "crop=floor(iw/2)*2:floor(ih/2)*2".into(),
            Self::FadeIn { duration } => format!("fade=t=in:st=0:d={duration}"),
            Self::FadeOut { duration, total } => {
                let st = (*total - *duration).max(0.0);
                format!("fade=t=out:st={st}:d={duration}")
            }
            Self::BlackAndWhite => "hue=s=0".into(),
        }
    }

    fn to_afilter(&self) -> Option<String> {
        match self {
            Self::Trim { start, duration } => Some(format!(
                "atrim=start={start}:duration={duration},asetpts=PTS-STARTPTS"
            )),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_chain() {
        let g = FilterGraph::new()
            .then(FilterOp::Trim {
                start: 1.0,
                duration: 2.0,
            })
            .then(FilterOp::HFlip)
            .then(FilterOp::Scale { w: 320, h: 180 });
        let vf = g.to_vf().unwrap();
        assert!(vf.contains("trim="));
        assert!(vf.contains("hflip"));
        assert!(vf.contains("scale=320:180"));
        let af = g.to_af().unwrap();
        assert!(af.contains("atrim=start=1"));
        assert!(af.contains("asetpts"));
    }

    #[test]
    fn no_audio_filter_without_trim() {
        let g = FilterGraph::new().then(FilterOp::HFlip);
        assert!(g.to_af().is_none());
    }
}
