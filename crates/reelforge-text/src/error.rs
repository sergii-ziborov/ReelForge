//! Text rendering errors.

use reelforge_core::CoreError;

/// Errors from text rendering.
#[derive(Debug, thiserror::Error)]
pub enum TextError {
    /// Underlying core model error.
    #[error(transparent)]
    Core(#[from] CoreError),

    /// Font load or glyph problem.
    #[error("font: {0}")]
    Font(String),

    /// Layout or sizing problem.
    #[error("layout: {0}")]
    Layout(String),

    /// Generic text error.
    #[error("text: {0}")]
    Message(String),
}

/// Result alias for text operations.
pub type Result<T> = std::result::Result<T, TextError>;

impl TextError {
    /// Font-related failure.
    #[must_use]
    pub fn font(message: impl Into<String>) -> Self {
        Self::Font(message.into())
    }

    /// Layout-related failure.
    #[must_use]
    pub fn layout(message: impl Into<String>) -> Self {
        Self::Layout(message.into())
    }

    /// Generic message.
    #[must_use]
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}
