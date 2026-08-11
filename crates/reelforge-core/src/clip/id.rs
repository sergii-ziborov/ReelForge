//! Stable clip identifiers.

use std::sync::Arc;

/// Stable identifier for nodes in an edit graph (optional bookkeeping).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClipId(Arc<str>);

impl ClipId {
    /// Create an id from a string.
    #[must_use]
    pub fn new(id: impl AsRef<str>) -> Self {
        Self(Arc::from(id.as_ref()))
    }

    /// Borrow the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ClipId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl std::fmt::Display for ClipId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
