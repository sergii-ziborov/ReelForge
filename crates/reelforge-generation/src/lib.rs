//! Sparse keyframes -> model-generated intermediate frames -> native composition.
//!
//! This crate executes an arbitrary shot plan through caller-provided backends.
//! It has no walking presets, embedded model weights or implicit cloud fallback.
//! RIFE/optical-flow interpolation is a separate capability and cannot satisfy a
//! request for new visual content. Model downloads, inference and native effects
//! stay separate; output receipts certify pixels/clock, never natural anatomy.

mod engine;
mod plan;
mod process;
mod sequence;
mod store;

pub use engine::{Engine, GenerationReceipt, Provider, Recovery};
pub use plan::{
    Backend, BackendKind, Clock, FrameRequest, GenerationPlan, GenerationSpec, ModelConstraints,
    Resources, ShotSpec, TaskKind, compile,
};
pub use process::{OutputFrame, ProcessConfig, ProcessProvider, ProcessRequest, ProcessResponse};
pub use sequence::GeneratedClip;
pub use store::{FrameReceipt, FrameStore};

/// Generation and qualification failure, before an incomplete clip is accepted.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid shot, clock, cache identity or frame coverage.
    #[error("invalid generation contract: {0}")]
    Invalid(String),
    /// No admitted backend can perform the requested generation.
    #[error("generation backend unavailable: {0}")]
    Unavailable(String),
    /// A reserved external request must be recovered without duplicate submission.
    #[error("generation outcome unknown; recover request {0}")]
    Unknown(String),
    /// A backend reported an execution failure.
    #[error("generation provider failed: {0}")]
    Provider(String),
    /// Storage or image decoding failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Durable request could not be decoded.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A result is not a supported image.
    #[error(transparent)]
    Image(#[from] image::ImageError),
}

/// Result returned by generation APIs.
pub type Result<T> = std::result::Result<T, Error>;
