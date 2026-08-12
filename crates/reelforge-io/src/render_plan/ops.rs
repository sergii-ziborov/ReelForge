//! Declarative plan operations and media sources.

use crate::FilterOp;
use serde::{Deserialize, Serialize};

/// Schema version for [`super::RenderPlan`] JSON documents.
pub const RENDER_PLAN_VERSION: u32 = 1;

/// Input media for a render plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlanSource {
    /// Host file path (UTF-8).
    File {
        /// Path to the input media.
        path: String,
    },
}

impl PlanSource {
    /// File source helper.
    #[must_use]
    pub fn file(path: impl Into<String>) -> Self {
        Self::File { path: path.into() }
    }

    /// Path when this is a file source.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::File { path } => Some(path.as_str()),
        }
    }
}

/// Where a plan op can execute after optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanBackend {
    /// Pure `FFmpeg` filtergraph (no Rust pixel import).
    Ffmpeg,
    /// In-process Rust clip graph / raster path.
    Rust,
    /// External adapter (e.g. vision tracks); not auto-extracted.
    Adapter,
}

/// One transform in a [`super::RenderPlan`].
///
/// Ops tagged [`PlanBackend::Ffmpeg`] form extractable prefixes. `Custom` and
/// adapter ops break extraction and force a Rust (or hybrid) remainder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlanOp {
    /// No-op (removed by the optimizer).
    Identity,
    /// Trim to `[start, start+duration)` seconds.
    Trim {
        /// Start seconds.
        start: f64,
        /// Length seconds.
        duration: f64,
    },
    /// Crop rectangle `x,y,w,h`.
    Crop {
        /// Left.
        x: u32,
        /// Top.
        y: u32,
        /// Width.
        w: u32,
        /// Height.
        h: u32,
    },
    /// Scale to exact width/height.
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
    /// Rotate 90° clockwise (`transpose=1`).
    TransposeCw,
    /// Force even dimensions (yuv420-friendly).
    EvenDims,
    /// Fade in from black.
    FadeIn {
        /// Fade length seconds.
        duration: f64,
    },
    /// Fade out to black (needs total media duration).
    FadeOut {
        /// Fade length seconds.
        duration: f64,
        /// Total media duration for start offset.
        total: f64,
    },
    /// Opaque custom / Rust-only op (breaks `FFmpeg` extraction).
    Custom {
        /// Stable name for agents and logs.
        name: String,
        /// Optional free-form parameters.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<serde_json::Value>,
    },
}

impl PlanOp {
    /// Preferred backend for this op.
    #[must_use]
    pub fn backend(&self) -> PlanBackend {
        match self {
            Self::Custom { name, .. } if is_ffmpeg_custom_name(name) => PlanBackend::Ffmpeg,
            Self::Custom { .. } => PlanBackend::Rust,
            Self::Identity
            | Self::Trim { .. }
            | Self::Crop { .. }
            | Self::Scale { .. }
            | Self::HFlip
            | Self::VFlip
            | Self::TransposeCw
            | Self::EvenDims
            | Self::FadeIn { .. }
            | Self::FadeOut { .. } => PlanBackend::Ffmpeg,
        }
    }

    /// Whether this op can run entirely inside an `FFmpeg` filtergraph.
    #[must_use]
    pub fn is_ffmpeg_capable(&self) -> bool {
        matches!(self.backend(), PlanBackend::Ffmpeg) && !matches!(self, Self::Identity)
    }

    /// Convert to [`FilterOp`] when `FFmpeg`-capable.
    ///
    /// # Errors
    ///
    /// Returns `None` for identity / custom ops.
    #[must_use]
    pub fn to_filter_op(&self) -> Option<FilterOp> {
        Some(match self {
            Self::Trim { start, duration } => FilterOp::Trim {
                start: *start,
                duration: *duration,
            },
            Self::Crop { x, y, w, h } => FilterOp::Crop {
                x: *x,
                y: *y,
                w: *w,
                h: *h,
            },
            Self::Scale { w, h } => FilterOp::Scale { w: *w, h: *h },
            Self::HFlip => FilterOp::HFlip,
            Self::VFlip => FilterOp::VFlip,
            Self::TransposeCw => FilterOp::TransposeCw,
            Self::EvenDims => FilterOp::EvenDims,
            Self::FadeIn { duration } => FilterOp::FadeIn {
                duration: *duration,
            },
            Self::FadeOut { duration, total } => FilterOp::FadeOut {
                duration: *duration,
                total: *total,
            },
            Self::Custom { name, .. } if is_ffmpeg_custom_name(name) => custom_to_filter_op(name)?,
            Self::Identity | Self::Custom { .. } => return None,
        })
    }
}

fn is_ffmpeg_custom_name(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "black_and_white" | "bw" | "grayscale" | "grey"
    )
}

fn custom_to_filter_op(name: &str) -> Option<FilterOp> {
    match name.trim().to_ascii_lowercase().as_str() {
        "black_and_white" | "bw" | "grayscale" | "grey" => Some(FilterOp::BlackAndWhite),
        _ => None,
    }
}

/// Optional encode / output settings attached to a plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PlanOutput {
    /// Destination path.
    pub path: String,
    /// Target fps when the runner needs it (optional for pure filtergraph).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    /// Video codec override (default libx264 via runner).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,
    /// CRF for software encoders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crf: Option<u8>,
}

impl PlanOutput {
    /// Output path helper.
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            fps: None,
            video_codec: None,
            crf: None,
        }
    }
}
