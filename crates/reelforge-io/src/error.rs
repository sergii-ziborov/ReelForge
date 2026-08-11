//! I/O error types.

use reelforge_core::CoreError;

/// Errors from media I/O operations.
#[derive(Debug, thiserror::Error)]
pub enum IoError {
    /// Underlying core model error.
    #[error(transparent)]
    Core(#[from] CoreError),

    /// `ffmpeg` / `ffprobe` binaries were not found.
    #[error("ffmpeg tools not found: {0}")]
    ToolsNotFound(String),

    /// An external process failed.
    #[error("ffmpeg process failed: {0}")]
    Process(String),

    /// Media probe or stream metadata is incomplete or invalid.
    #[error("probe: {0}")]
    Probe(String),

    /// Path or resource problem.
    #[error("io: {0}")]
    Message(String),

    /// Image crate failure.
    #[error("image: {0}")]
    Image(String),
}

/// Result alias for I/O operations.
pub type Result<T> = std::result::Result<T, IoError>;

impl IoError {
    /// Build a process error from a detail string.
    #[must_use]
    pub fn process(message: impl Into<String>) -> Self {
        Self::Process(message.into())
    }

    /// Build a probe error.
    #[must_use]
    pub fn probe(message: impl Into<String>) -> Self {
        Self::Probe(message.into())
    }

    /// Build a generic message error.
    #[must_use]
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    /// Build an image error.
    #[must_use]
    pub fn image(message: impl Into<String>) -> Self {
        Self::Image(message.into())
    }
}
