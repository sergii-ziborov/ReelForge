//! Error types for the core media model.

use crate::time::{Duration, Time};

/// Fallible core operations return this result alias.
pub type Result<T> = std::result::Result<T, CoreError>;

/// Errors produced by core clip, frame, and audio operations.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CoreError {
    /// Requested sample time lies outside the clip's active range.
    #[error("time {time} is outside clip range {range:?}")]
    TimeOutOfRange {
        /// Requested media time.
        time: Time,
        /// Clip range that was valid.
        range: (Time, Time),
    },

    /// Dimensions or buffer length do not match the frame format.
    #[error("invalid frame geometry: {message}")]
    InvalidFrame {
        /// Human-readable detail.
        message: String,
    },

    /// Audio buffer layout is inconsistent with the declared format.
    #[error("invalid audio buffer: {message}")]
    InvalidAudio {
        /// Human-readable detail.
        message: String,
    },

    /// A duration or range is empty or inverted.
    #[error("invalid duration or range: {message}")]
    InvalidTiming {
        /// Human-readable detail.
        message: String,
    },

    /// Subclip bounds do not fit inside the parent clip.
    #[error("subclip {requested:?} exceeds parent duration {parent}")]
    SubclipOutOfBounds {
        /// Requested subclip range relative to the parent.
        requested: (Time, Duration),
        /// Parent clip duration.
        parent: Duration,
    },

    /// Operation requires a positive size.
    #[error("size must be positive, got {0:?}")]
    InvalidSize(crate::layout::Size),

    /// Generic unsupported operation at this stage of the stack.
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
}

impl CoreError {
    /// Convenience constructor for invalid frame geometry.
    #[must_use]
    pub fn invalid_frame(message: impl Into<String>) -> Self {
        Self::InvalidFrame {
            message: message.into(),
        }
    }

    /// Convenience constructor for invalid audio buffers.
    #[must_use]
    pub fn invalid_audio(message: impl Into<String>) -> Self {
        Self::InvalidAudio {
            message: message.into(),
        }
    }

    /// Convenience constructor for timing problems.
    #[must_use]
    pub fn invalid_timing(message: impl Into<String>) -> Self {
        Self::InvalidTiming {
            message: message.into(),
        }
    }
}
