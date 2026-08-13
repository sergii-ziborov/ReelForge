//! Errors for render-graph construction and validation.

use thiserror::Error;

/// Result alias.
pub type Result<T> = std::result::Result<T, GraphError>;

/// Stable diagnostic codes (agents / CI may match these).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GraphErrorCode {
    /// Generic validation failure.
    Message,
    /// Unknown node or asset id.
    UnknownId,
    /// Cycle in the DAG.
    Cycle,
    /// Operation not registered.
    UnknownOperation,
    /// Duplicate node id.
    DuplicateNodeId,
    /// Duplicate asset id.
    DuplicateAssetId,
    /// Duplicate output name.
    DuplicateOutputName,
    /// Parameter / schema validation failed.
    InvalidParams,
    /// Operation registered but not executable.
    NotExecutable,
    /// Media contract mismatch between nodes.
    MediaContract,
}

impl GraphErrorCode {
    /// Stable string code for logs / agents.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "RFGRAPH_MESSAGE",
            Self::UnknownId => "RFGRAPH_UNKNOWN_ID",
            Self::Cycle => "RFGRAPH_CYCLE",
            Self::UnknownOperation => "RFGRAPH_UNKNOWN_OPERATION",
            Self::DuplicateNodeId => "RFGRAPH_DUPLICATE_NODE_ID",
            Self::DuplicateAssetId => "RFGRAPH_DUPLICATE_ASSET_ID",
            Self::DuplicateOutputName => "RFGRAPH_DUPLICATE_OUTPUT_NAME",
            Self::InvalidParams => "RFGRAPH_INVALID_PARAMS",
            Self::NotExecutable => "RFGRAPH_NOT_EXECUTABLE",
            Self::MediaContract => "RFGRAPH_MEDIA_CONTRACT",
        }
    }
}

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
    /// Duplicate node id (first and second occurrence indices).
    #[error("duplicate node id `{id}` (indices {first_index} and {second_index})")]
    DuplicateNodeId {
        /// Node id string.
        id: String,
        /// First occurrence in `nodes` array.
        first_index: usize,
        /// Second occurrence in `nodes` array.
        second_index: usize,
    },
    /// Duplicate asset id.
    #[error("duplicate asset id `{id}` (indices {first_index} and {second_index})")]
    DuplicateAssetId {
        /// Asset id string.
        id: String,
        /// First occurrence in `assets` array.
        first_index: usize,
        /// Second occurrence in `assets` array.
        second_index: usize,
    },
    /// Duplicate output name.
    #[error("duplicate output name `{name}` (indices {first_index} and {second_index})")]
    DuplicateOutputName {
        /// Output name.
        name: String,
        /// First occurrence in `outputs` array.
        first_index: usize,
        /// Second occurrence in `outputs` array.
        second_index: usize,
    },
    /// Parameter validation failed for an operation.
    #[error("invalid params for `{operation}`: {message}")]
    InvalidParams {
        /// Operation id.
        operation: String,
        /// Human detail.
        message: String,
    },
    /// Registered but no executor implementation.
    #[error("operation `{0}` is registered but has no executor")]
    NotExecutable(String),
    /// Input/output media contract mismatch.
    #[error("media contract: {0}")]
    MediaContract(String),
}

impl GraphError {
    /// Message helper.
    #[must_use]
    pub fn message(msg: impl Into<String>) -> Self {
        Self::Message(msg.into())
    }

    /// Stable diagnostic code.
    #[must_use]
    pub const fn code(&self) -> GraphErrorCode {
        match self {
            Self::Message(_) => GraphErrorCode::Message,
            Self::UnknownId(_) => GraphErrorCode::UnknownId,
            Self::Cycle => GraphErrorCode::Cycle,
            Self::UnknownOperation(_) => GraphErrorCode::UnknownOperation,
            Self::DuplicateNodeId { .. } => GraphErrorCode::DuplicateNodeId,
            Self::DuplicateAssetId { .. } => GraphErrorCode::DuplicateAssetId,
            Self::DuplicateOutputName { .. } => GraphErrorCode::DuplicateOutputName,
            Self::InvalidParams { .. } => GraphErrorCode::InvalidParams,
            Self::NotExecutable(_) => GraphErrorCode::NotExecutable,
            Self::MediaContract(_) => GraphErrorCode::MediaContract,
        }
    }

    /// Code as string (`RFGRAPH_*`).
    #[must_use]
    pub const fn code_str(&self) -> &'static str {
        self.code().as_str()
    }
}
