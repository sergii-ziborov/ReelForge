//! SightLoom-shaped track document (JSON only — no vision crate).

use serde::{Deserialize, Serialize};

/// Wire format version for adapter documents.
pub const TRACK_DOC_VERSION: u32 = 1;

/// JSON document: one or more tracker trajectories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackDocument {
    /// Schema version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Tracks.
    #[serde(default)]
    pub tracks: Vec<TrackEntry>,
}

fn default_version() -> u32 {
    TRACK_DOC_VERSION
}

/// One track in the document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackEntry {
    /// Tracker id (`TrackId`).
    pub id: String,
    /// Optional subject handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Optional kind (`face`, `person`, …) stored as provenance model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Timed samples.
    #[serde(default)]
    pub samples: Vec<SampleEntry>,
}

/// One sample: ellipse, `x/y/w/h`, or `left/top/right/bottom`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleEntry {
    /// Time in seconds.
    pub t: f64,
    /// Center X.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cx: Option<f32>,
    /// Center Y.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cy: Option<f32>,
    /// Radius.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    /// Box origin X.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f32>,
    /// Box origin Y.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f32>,
    /// Box width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<f32>,
    /// Box height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<f32>,
    /// Box left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<f32>,
    /// Box top.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top: Option<f32>,
    /// Box right.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<f32>,
    /// Box bottom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom: Option<f32>,
    /// Confidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conf: Option<f32>,
    /// Occlusion (`visible` / `partial` / `occluded` / `unknown`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occlusion: Option<String>,
    /// Appearance id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<String>,
    /// Observation id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<String>,
    /// Compact-mask handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<MaskEntry>,
}

/// Optional mask sidecar (`MaskRef`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaskEntry {
    /// Observation id (defaults to the sample's observation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<String>,
    /// Adapter-defined URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}
