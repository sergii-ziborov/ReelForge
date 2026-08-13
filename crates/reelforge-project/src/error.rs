//! Project / compile errors.

/// Fallible project operations.
pub type Result<T> = std::result::Result<T, ProjectError>;

/// Schema or compile failure.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ProjectError {
    /// Unsupported or unknown schema version.
    #[error("capture project version {0} is not supported")]
    Version(u32),
    /// Structural problem (missing sequence, bad media ref, …).
    #[error("capture project: {0}")]
    Message(String),
    /// Graph produced by compile failed validation.
    #[error("compiled RenderGraph: {0}")]
    Graph(String),
}

impl ProjectError {
    pub(crate) fn message(msg: impl Into<String>) -> Self {
        Self::Message(msg.into())
    }
}
