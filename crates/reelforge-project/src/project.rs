//! [`CaptureProject`] document (user timeline — not a `RenderGraph`).

use crate::error::{ProjectError, Result};
use crate::ids::{ProjectId, SequenceId, TimelineTrackId};
use crate::model::{Marker, MediaRef, Metadata, SemanticRef, TimelineItem};
use serde::{Deserialize, Serialize};

/// Current `CaptureProject` schema.
pub const CAPTURE_PROJECT_VERSION: u32 = 1;

/// Kind of timeline track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// Picture.
    Video,
    /// Sound.
    Audio,
    /// Captions.
    Subtitle,
}

/// One OTIO-like track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineTrack {
    /// Id.
    pub id: TimelineTrackId,
    /// Video / audio / subtitle.
    pub kind: TrackKind,
    /// Items in record order.
    #[serde(default)]
    pub items: Vec<TimelineItem>,
    /// Soft mute (compile may skip audio).
    #[serde(default)]
    pub muted: bool,
}

impl TimelineTrack {
    /// Empty track.
    #[must_use]
    pub fn new(id: TimelineTrackId, kind: TrackKind) -> Self {
        Self {
            id,
            kind,
            items: Vec::new(),
            muted: false,
        }
    }
}

/// One sequence (a timeline).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sequence {
    /// Id.
    pub id: SequenceId,
    /// Display name.
    pub name: String,
    /// Tracks (bottom → top for video).
    #[serde(default)]
    pub tracks: Vec<TimelineTrack>,
    /// Sequence markers.
    #[serde(default)]
    pub markers: Vec<Marker>,
    /// Optional compose canvas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas: Option<(u32, u32)>,
}

impl Sequence {
    /// Empty named sequence.
    #[must_use]
    pub fn new(id: SequenceId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            tracks: Vec::new(),
            markers: Vec::new(),
            canvas: None,
        }
    }
}

/// User-facing project. Compiles to [`reelforge_render_graph::RenderGraph`].
///
/// Editor UX, autosave policy, and screen capture stay in `ReelForge` Capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureProject {
    /// Schema version.
    pub version: u32,
    /// Project id.
    pub id: ProjectId,
    /// Display name.
    pub name: String,
    /// Sequences.
    #[serde(default)]
    pub sequences: Vec<Sequence>,
    /// Active sequence (default: first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_sequence: Option<SequenceId>,
    /// Media library.
    #[serde(default)]
    pub media: Vec<MediaRef>,
    /// Project-level markers.
    #[serde(default)]
    pub markers: Vec<Marker>,
    /// Metadata.
    #[serde(default)]
    pub metadata: Metadata,
    /// Semantic references (Intelligence / `SightLoom` ids).
    #[serde(default)]
    pub semantic: Vec<SemanticRef>,
}

impl CaptureProject {
    /// Empty v1 project.
    #[must_use]
    pub fn new(id: ProjectId, name: impl Into<String>) -> Self {
        Self {
            version: CAPTURE_PROJECT_VERSION,
            id,
            name: name.into(),
            sequences: Vec::new(),
            active_sequence: None,
            media: Vec::new(),
            markers: Vec::new(),
            metadata: Metadata::default(),
            semantic: Vec::new(),
        }
    }

    /// Bump legacy versions to current. Unknown future versions error.
    ///
    /// # Errors
    ///
    /// Version newer than [`CAPTURE_PROJECT_VERSION`].
    pub fn migrate(mut self) -> Result<Self> {
        if self.version == 0 {
            self.version = CAPTURE_PROJECT_VERSION;
        }
        if self.version > CAPTURE_PROJECT_VERSION {
            return Err(ProjectError::Version(self.version));
        }
        Ok(self)
    }

    /// Parse JSON (migrates `version: 0`).
    ///
    /// # Errors
    ///
    /// JSON or version.
    pub fn from_json(text: &str) -> Result<Self> {
        let p: Self =
            serde_json::from_str(text).map_err(|e| ProjectError::message(format!("json: {e}")))?;
        p.migrate()
    }

    /// Pretty JSON.
    ///
    /// # Errors
    ///
    /// Serde.
    pub fn to_json_pretty(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| ProjectError::message(format!("json: {e}")))
    }

    /// Sequence used for compile.
    ///
    /// # Errors
    ///
    /// Missing sequences / unknown active id.
    pub fn active(&self) -> Result<&Sequence> {
        if self.sequences.is_empty() {
            return Err(ProjectError::message("project has no sequences"));
        }
        if let Some(id) = &self.active_sequence {
            return self
                .sequences
                .iter()
                .find(|s| &s.id == id)
                .ok_or_else(|| ProjectError::message(format!("unknown sequence {}", id.as_str())));
        }
        Ok(&self.sequences[0])
    }
}
