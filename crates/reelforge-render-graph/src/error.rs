//! Errors for render-graph construction and validation.

use thiserror::Error;

/// Result alias.
pub type Result<T> = std::result::Result<T, GraphError>;

/// Graph / contract errors.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum GraphError {
    /// Generic validation failure.
    #[error("render graph: {0}")]
    Message(String),
    /// Unknown node or asset id.
    #[error("unknown id: {0}")]
    UnknownId(String),
    /// Cycle detected in the DAG.
    #[error("cycle detected in render graph")]
    Cycle,
    /// Operation not registered.
    #[error("unknown operation: {0}")]
    UnknownOperation(String),
}

impl GraphError {
    /// Message helper.
    #[must_use]
    pub fn message(msg: impl Into<String>) -> Self {
        Self::Message(msg.into())
    }
}
