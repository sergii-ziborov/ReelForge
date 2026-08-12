//! Typed operation registry (MCP / agents — not open `Custom` JSON forever).

use crate::error::{GraphError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Stable operation identifier (e.g. `rf.redaction.region`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OperationId(pub String);

impl OperationId {
    /// Construct from string.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// As str.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for OperationId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

/// Semantic version for an operation schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemVer {
    /// Major.
    pub major: u32,
    /// Minor.
    pub minor: u32,
    /// Patch.
    pub patch: u32,
}

impl SemVer {
    /// `1.0.0`.
    pub const V1: Self = Self {
        major: 1,
        minor: 0,
        patch: 0,
    };

    /// Construct.
    #[must_use]
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl core::fmt::Display for SemVer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Preferred execution backend class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendClass {
    /// Host `FFmpeg` filtergraph / encode.
    Ffmpeg,
    /// In-process Rust raster/audio.
    Rust,
    /// External adapter (e.g. `SightLoom` materialization).
    Adapter,
    /// GPU path (future).
    Gpu,
}

/// High-level media kind contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MediaContract {
    /// Accepts video frames.
    #[serde(default)]
    pub video: bool,
    /// Accepts audio.
    #[serde(default)]
    pub audio: bool,
    /// Accepts mask timelines.
    #[serde(default)]
    pub masks: bool,
    /// Free-form notes / pixel formats later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Capability flags for feature detection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CapabilitySet {
    /// Tags such as `realtime`, `privacy`, `preview`.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Soft resource limits for an operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct OperationLimits {
    /// Max recommended resolution width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<u32>,
    /// Max recommended resolution height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<u32>,
    /// Notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Full operation descriptor for registry / MCP.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationDescriptor {
    /// Stable id.
    pub id: OperationId,
    /// Schema version.
    pub version: SemVer,
    /// Input contract.
    pub input: MediaContract,
    /// Output contract.
    pub output: MediaContract,
    /// Backend preference.
    pub backend: BackendClass,
    /// Bit-reproducible given same inputs/backends.
    pub deterministic: bool,
    /// Capability tags.
    #[serde(default)]
    pub capabilities: CapabilitySet,
    /// JSON Schema fragment for parameters (free-form object for M0).
    #[serde(default)]
    pub parameter_schema: Value,
    /// Limits.
    #[serde(default)]
    pub limits: OperationLimits,
}

/// In-memory registry of typed operations.
#[derive(Debug, Clone, Default)]
pub struct OperationRegistry {
    ops: BTreeMap<OperationId, OperationDescriptor>,
}

impl OperationRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builtin `ReelForge` M0/M2 ops.
    #[must_use]
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        r.register(OperationDescriptor {
            id: OperationId::new("rf.transform.trim"),
            version: SemVer::V1,
            input: MediaContract {
                video: true,
                audio: true,
                masks: false,
                notes: None,
            },
            output: MediaContract {
                video: true,
                audio: true,
                masks: false,
                notes: None,
            },
            backend: BackendClass::Ffmpeg,
            deterministic: true,
            capabilities: CapabilitySet {
                tags: vec!["edit".into()],
            },
            parameter_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "start": { "type": "object" },
                    "duration": { "type": "object" }
                }
            }),
            limits: OperationLimits::default(),
        });
        r.register(OperationDescriptor {
            id: OperationId::new("rf.redaction.region"),
            version: SemVer::V1,
            input: MediaContract {
                video: true,
                audio: false,
                masks: true,
                notes: Some("RegionRedaction + MaskTimeline".into()),
            },
            output: MediaContract {
                video: true,
                audio: false,
                masks: false,
                notes: None,
            },
            backend: BackendClass::Rust,
            deterministic: true,
            capabilities: CapabilitySet {
                tags: vec!["privacy".into(), "vision".into()],
            },
            parameter_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "style": { "type": "object" },
                    "masks": { "type": "object" }
                }
            }),
            limits: OperationLimits::default(),
        });
        r.register(OperationDescriptor {
            id: OperationId::new("rf.encode.h264"),
            version: SemVer::V1,
            input: MediaContract {
                video: true,
                audio: true,
                masks: false,
                notes: None,
            },
            output: MediaContract {
                video: true,
                audio: true,
                masks: false,
                notes: Some("container file".into()),
            },
            backend: BackendClass::Ffmpeg,
            deterministic: false,
            capabilities: CapabilitySet {
                tags: vec!["encode".into()],
            },
            parameter_schema: serde_json::json!({
                "type": "object",
                "properties": { "crf": { "type": "integer" }, "path": { "type": "string" } }
            }),
            limits: OperationLimits::default(),
        });
        r
    }

    /// Insert or replace.
    pub fn register(&mut self, desc: OperationDescriptor) {
        self.ops.insert(desc.id.clone(), desc);
    }

    /// Lookup.
    ///
    /// # Errors
    ///
    /// Unknown id.
    pub fn get(&self, id: &OperationId) -> Result<&OperationDescriptor> {
        self.ops
            .get(id)
            .ok_or_else(|| GraphError::UnknownOperation(id.0.clone()))
    }

    /// All ids.
    pub fn ids(&self) -> impl Iterator<Item = &OperationId> {
        self.ops.keys()
    }

    /// Number of ops.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// Empty check.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_contain_redaction() {
        let r = OperationRegistry::with_builtins();
        assert!(r.len() >= 3);
        let d = r
            .get(&OperationId::new("rf.redaction.region"))
            .unwrap();
        assert_eq!(d.backend, BackendClass::Rust);
    }
}
