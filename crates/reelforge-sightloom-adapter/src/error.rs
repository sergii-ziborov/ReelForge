//! Adapter errors (JSON / sample shape only).

/// Fallible adapter operations.
pub type Result<T> = std::result::Result<T, AdapterError>;

/// Errors from parsing a SightLoom-shaped track document.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AdapterError {
    /// Invalid JSON.
    #[error("track document json: {0}")]
    Json(String),
    /// A sample is missing geometry or has a bad field.
    #[error("track sample: {0}")]
    Sample(String),
    /// Filesystem error.
    #[error("track document io: {0}")]
    Io(String),
}
