//! `CaptureProject`: OTIO-like user timeline that **compiles** to `RenderGraph`.
//!
//! This is the project *schema* + compiler. Screen capture, desktop editor,
//! and cache policy belong to `ReelForge` Capture — not this crate.

mod compile;
mod emit;
mod emit_clip;
mod error;
mod ids;
mod model;
mod project;

pub use compile::{ProjectCompile, compile_project};
pub use error::{ProjectError, Result};
pub use ids::{MediaRefId, ProjectId, SequenceId, TimelineClipId, TimelineTrackId};
pub use model::{
    CropRect, Gap, Marker, MediaRef, Metadata, NestedSequence, Retiming, SemanticRef, SourceRange,
    TimelineClip, TimelineItem, Transition, TransitionKind,
};
pub use project::{CAPTURE_PROJECT_VERSION, CaptureProject, Sequence, TimelineTrack, TrackKind};
