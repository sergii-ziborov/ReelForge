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

impl From<&str> for OperationId {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

impl From<String> for OperationId {
    fn from(id: String) -> Self {
        Self::new(id)
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

/// How the I/O runner gathers inputs for [`crate::compile::compile_op`] + execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorKind {
    /// One upstream media product.
    #[default]
    Unary,
    /// All upstream products (`compose`, `mix`).
    Nary,
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
///
/// `#[non_exhaustive]` so 0.2.x can add optional streams without another `SemVer` break.
/// Construct with [`MediaContract::video_av`], [`MediaContract::video_only`], or
/// [`MediaContract::audio_only`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
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

impl MediaContract {
    /// Video + companion audio (typical file source / transform passthrough).
    #[must_use]
    pub const fn video_av() -> Self {
        Self {
            video: true,
            audio: true,
            masks: false,
            notes: None,
        }
    }

    /// Video only (no audio).
    #[must_use]
    pub const fn video_only() -> Self {
        Self {
            video: true,
            audio: false,
            masks: false,
            notes: None,
        }
    }

    /// Audio only.
    #[must_use]
    pub const fn audio_only() -> Self {
        Self {
            video: false,
            audio: true,
            masks: false,
            notes: None,
        }
    }

    /// Whether `self` supplies every stream `required` asks for.
    #[must_use]
    pub fn satisfies(&self, required: &Self) -> bool {
        (!required.video || self.video)
            && (!required.audio || self.audio)
            && (!required.masks || self.masks)
    }

    /// Drop notes (contracts on the compiled program stay data-only).
    #[must_use]
    pub fn without_notes(&self) -> Self {
        Self {
            notes: None,
            ..self.clone()
        }
    }

    /// Attach a free-form note.
    #[must_use]
    pub fn with_notes(mut self, notes: impl Into<String>) -> Self {
        self.notes = Some(notes.into());
        self
    }

    /// Mark the contract as accepting mask timelines.
    #[must_use]
    pub fn with_masks(mut self) -> Self {
        self.masks = true;
        self
    }
}

/// Capability flags for feature detection.
///
/// Construct with [`CapabilitySet::from_tags`] from outside this crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub struct CapabilitySet {
    /// Tags such as `realtime`, `privacy`, `preview`.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl CapabilitySet {
    /// Tags in registration order.
    #[must_use]
    pub fn from_tags<S>(tags: impl IntoIterator<Item = S>) -> Self
    where
        S: Into<String>,
    {
        Self {
            tags: tags.into_iter().map(Into::into).collect(),
        }
    }
}

/// Soft resource limits for an operation.
///
/// Extra limit fields may appear in 0.2.x; construct with [`Default`] or
/// [`OperationLimits::new`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[non_exhaustive]
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

impl OperationLimits {
    /// Empty limits.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Full operation descriptor for registry / MCP.
///
/// **0.2 contract:** this type is `#[non_exhaustive]`. Downstream crates
/// (Intelligence, Capture, MCP hosts) must construct it with
/// [`OperationDescriptor::new`] — not a struct literal. JSON still accepts
/// omitted `executor_kind` (`#[serde(default)]` → [`ExecutorKind::Unary`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
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
    /// Input arity for the bound executor (`execute` in `reelforge-io`).
    #[serde(default)]
    pub executor_kind: ExecutorKind,
}

impl OperationDescriptor {
    /// Registerable descriptor. Extra fields default to deterministic, unary,
    /// empty capabilities / schema / limits.
    ///
    /// This is the stable constructor for 0.2.x. New optional fields will land
    /// here with defaults — do not write struct literals in other crates.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        version: SemVer,
        input: MediaContract,
        output: MediaContract,
        backend: BackendClass,
    ) -> Self {
        Self {
            id: OperationId::new(id),
            version,
            input,
            output,
            backend,
            deterministic: true,
            capabilities: CapabilitySet::default(),
            parameter_schema: Value::Null,
            limits: OperationLimits::default(),
            executor_kind: ExecutorKind::Unary,
        }
    }

    /// Unary shortcut (`new` already defaults to unary).
    #[must_use]
    pub fn unary(
        id: impl Into<String>,
        version: SemVer,
        input: MediaContract,
        output: MediaContract,
        backend: BackendClass,
    ) -> Self {
        Self::new(id, version, input, output, backend)
    }

    /// N-ary shortcut (`compose`, `mix`).
    #[must_use]
    pub fn nary(
        id: impl Into<String>,
        version: SemVer,
        input: MediaContract,
        output: MediaContract,
        backend: BackendClass,
    ) -> Self {
        Self::new(id, version, input, output, backend).with_executor_kind(ExecutorKind::Nary)
    }

    /// Override bit-reproducibility (encoders are typically `false`).
    #[must_use]
    pub fn with_deterministic(mut self, deterministic: bool) -> Self {
        self.deterministic = deterministic;
        self
    }

    /// Replace capability tags.
    #[must_use]
    pub fn with_capabilities<S>(mut self, tags: impl IntoIterator<Item = S>) -> Self
    where
        S: Into<String>,
    {
        self.capabilities = CapabilitySet::from_tags(tags);
        self
    }

    /// JSON Schema fragment for parameters.
    #[must_use]
    pub fn with_parameter_schema(mut self, schema: Value) -> Self {
        self.parameter_schema = schema;
        self
    }

    /// Soft resource limits.
    #[must_use]
    pub fn with_limits(mut self, limits: OperationLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Input arity for the bound executor.
    #[must_use]
    pub fn with_executor_kind(mut self, kind: ExecutorKind) -> Self {
        self.executor_kind = kind;
        self
    }
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

    /// Builtin `ReelForge` M0–M3 ops.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        let video_av = MediaContract::video_av();
        let video_only = MediaContract::video_only();
        let mk = |id: &str,
                  backend: BackendClass,
                  input: MediaContract,
                  output: MediaContract,
                  schema: Value,
                  tags: &[&str],
                  kind: ExecutorKind| {
            OperationDescriptor::new(id, SemVer::V1, input, output, backend)
                .with_parameter_schema(schema)
                .with_capabilities(tags.iter().copied())
                .with_executor_kind(kind)
        };

        let mut transform = |id: &str, backend: BackendClass, schema: Value, tags: &[&str]| {
            r.register(mk(
                id,
                backend,
                video_av.clone(),
                video_av.clone(),
                schema,
                tags,
                ExecutorKind::Unary,
            ));
        };

        transform(
            "rf.transform.trim",
            BackendClass::Ffmpeg,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "start": {},
                    "duration": {}
                }
            }),
            &["edit"],
        );
        transform(
            "rf.transform.hflip",
            BackendClass::Ffmpeg,
            serde_json::json!({ "type": "object" }),
            &["edit"],
        );
        transform(
            "rf.transform.vflip",
            BackendClass::Ffmpeg,
            serde_json::json!({ "type": "object" }),
            &["edit"],
        );
        transform(
            "rf.transform.scale",
            BackendClass::Ffmpeg,
            serde_json::json!({
                "type": "object",
                "properties": { "w": { "type": "integer" }, "h": { "type": "integer" } }
            }),
            &["edit"],
        );
        transform(
            "rf.transform.crop",
            BackendClass::Ffmpeg,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "w": { "type": "integer" },
                    "h": { "type": "integer" }
                }
            }),
            &["edit"],
        );
        transform(
            "rf.transform.even_dims",
            BackendClass::Ffmpeg,
            serde_json::json!({ "type": "object" }),
            &["edit"],
        );
        transform(
            "rf.transform.rotate",
            BackendClass::Rust,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "degrees": { "type": "number" },
                    "mode": { "type": "string" }
                }
            }),
            &["edit"],
        );
        transform(
            "rf.transform.fade_in",
            BackendClass::Rust,
            serde_json::json!({
                "type": "object",
                "properties": { "duration": {} }
            }),
            &["edit", "fade"],
        );
        transform(
            "rf.transform.fade_out",
            BackendClass::Rust,
            serde_json::json!({
                "type": "object",
                "properties": { "duration": {} }
            }),
            &["edit", "fade"],
        );
        transform(
            "rf.transform.speed",
            BackendClass::Rust,
            serde_json::json!({
                "type": "object",
                "properties": { "factor": { "type": "number" } },
                "required": ["factor"]
            }),
            &["edit", "time"],
        );
        transform(
            "rf.transform.freeze",
            BackendClass::Rust,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "at": {},
                    "hold": {}
                },
                "required": ["hold"]
            }),
            &["edit", "time"],
        );
        transform(
            "rf.transform.loop",
            BackendClass::Rust,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "duration": {},
                    "times": { "type": "integer" },
                    "n": { "type": "integer" }
                }
            }),
            &["edit", "time"],
        );
        transform(
            "rf.transform.slide_in",
            BackendClass::Rust,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "duration": {},
                    "side": { "type": "string" }
                },
                "required": ["duration"]
            }),
            &["edit", "transition"],
        );
        transform(
            "rf.transform.slide_out",
            BackendClass::Rust,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "duration": {},
                    "side": { "type": "string" }
                },
                "required": ["duration"]
            }),
            &["edit", "transition"],
        );

        r.register(mk(
            "rf.color.black_and_white",
            BackendClass::Rust,
            video_only.clone(),
            video_only.clone(),
            serde_json::json!({ "type": "object" }),
            &["color"],
            ExecutorKind::Unary,
        ));
        r.register(mk(
            "rf.color.invert",
            BackendClass::Rust,
            video_only.clone(),
            video_only.clone(),
            serde_json::json!({ "type": "object" }),
            &["color"],
            ExecutorKind::Unary,
        ));
        r.register(mk(
            "rf.color.painting",
            BackendClass::Rust,
            video_only.clone(),
            video_only.clone(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "saturation": { "type": "number" },
                    "black": { "type": "number" }
                }
            }),
            &["color", "stylize"],
            ExecutorKind::Unary,
        ));
        r.register(mk(
            "rf.compose.layers",
            BackendClass::Rust,
            MediaContract::video_only().with_notes("multi-input composite"),
            MediaContract::video_only(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "w": { "type": "integer" },
                    "h": { "type": "integer" },
                    "layers": { "type": "array" }
                }
            }),
            &["compose"],
            ExecutorKind::Nary,
        ));
        r.register(mk(
            "rf.timeline.concat",
            BackendClass::Rust,
            MediaContract::video_only().with_notes("n-ary sequential concat"),
            MediaContract::video_only().with_notes("duration = sum of inputs"),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "clips": { "type": "array" },
                    "ranges": { "type": "array" }
                }
            }),
            &["timeline", "concat", "edit"],
            ExecutorKind::Nary,
        ));
        r.register(mk(
            "rf.audio.gain",
            BackendClass::Rust,
            MediaContract::audio_only(),
            MediaContract::audio_only(),
            serde_json::json!({
                "type": "object",
                "properties": { "factor": { "type": "number" } }
            }),
            &["audio"],
            ExecutorKind::Unary,
        ));
        r.register(mk(
            "rf.audio.drop",
            BackendClass::Rust,
            video_av.clone(),
            video_only.clone(),
            serde_json::json!({ "type": "object" }),
            &["audio"],
            ExecutorKind::Unary,
        ));
        r.register(mk(
            "rf.audio.preserve",
            BackendClass::Rust,
            video_av.clone(),
            video_av.clone(),
            serde_json::json!({ "type": "object" }),
            &["audio"],
            ExecutorKind::Unary,
        ));
        r.register(mk(
            "rf.audio.mix",
            BackendClass::Rust,
            MediaContract::video_av().with_notes("multi-input audio mix; video from first input"),
            video_av.clone(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "tracks": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "gain": { "type": "number" },
                                "start": { "type": "number" }
                            }
                        }
                    }
                }
            }),
            &["audio", "mix"],
            ExecutorKind::Nary,
        ));
        r.register(mk(
            "rf.redaction.region",
            BackendClass::Rust,
            MediaContract::video_only()
                .with_masks()
                .with_notes("RegionRedaction + MaskTimeline"),
            MediaContract::video_only(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "style": { "type": "object" },
                    "masks": { "type": "object" }
                }
            }),
            &["privacy", "vision"],
            ExecutorKind::Unary,
        ));
        r.register(mk(
            "rf.subtitle.burn",
            BackendClass::Rust,
            MediaContract::video_only(),
            MediaContract::video_only(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "cues": { "type": "array" }
                }
            }),
            &["text", "subtitle"],
            ExecutorKind::Unary,
        ));
        r.register(mk(
            "rf.adapter.sightloom",
            BackendClass::Adapter,
            MediaContract::video_av().with_notes("SightLoom materialize → MaskTimeline"),
            MediaContract::video_av()
                .with_masks()
                .with_notes("video passthrough + masks"),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "tracks": {},
                    "masks": {},
                    "document": {},
                    "package_id": { "type": "string" },
                    "query": {}
                }
            }),
            &["adapter", "vision", "privacy"],
            ExecutorKind::Unary,
        ));
        r.register(mk(
            "rf.gpu.passthrough",
            BackendClass::Gpu,
            video_av.clone(),
            video_av.clone(),
            serde_json::json!({
                "type": "object",
                "properties": { "backend": { "type": "string" } }
            }),
            &["gpu"],
            ExecutorKind::Unary,
        ));
        r.register(
            mk(
                "rf.encode.hw",
                BackendClass::Gpu,
                video_av.clone(),
                MediaContract::video_av().with_notes("hardware encode hint"),
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "backend": { "type": "string" },
                        "codec": { "type": "string" }
                    }
                }),
                &["gpu", "encode"],
                ExecutorKind::Unary,
            )
            .with_deterministic(false),
        );
        r.register(
            mk(
                "rf.encode.h264",
                BackendClass::Ffmpeg,
                video_av.clone(),
                MediaContract::video_av().with_notes("container file"),
                serde_json::json!({
                    "type": "object",
                    "properties": { "crf": { "type": "integer" }, "path": { "type": "string" } }
                }),
                &["encode"],
                ExecutorKind::Unary,
            )
            .with_deterministic(false),
        );
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
        let d = r.get(&OperationId::new("rf.redaction.region")).unwrap();
        assert_eq!(d.backend, BackendClass::Rust);
        assert_eq!(d.executor_kind, ExecutorKind::Unary);
        assert!(d.input.masks);
    }

    #[test]
    fn constructor_defaults_executor_kind_unary() {
        let d = OperationDescriptor::unary(
            "rf.custom.demo",
            SemVer::V1,
            MediaContract::video_only(),
            MediaContract::video_only(),
            BackendClass::Rust,
        );
        assert_eq!(d.executor_kind, ExecutorKind::Unary);
        assert!(d.deterministic);
        assert!(d.capabilities.tags.is_empty());
        assert_eq!(d.parameter_schema, Value::Null);
    }

    #[test]
    fn nary_builder_and_schema_roundtrip() {
        let d = OperationDescriptor::nary(
            "rf.custom.mix",
            SemVer::V1,
            MediaContract::video_av(),
            MediaContract::video_av(),
            BackendClass::Rust,
        )
        .with_capabilities(["mix"])
        .with_parameter_schema(serde_json::json!({ "type": "object" }))
        .with_limits(OperationLimits::new());
        assert_eq!(d.executor_kind, ExecutorKind::Nary);
        assert_eq!(d.capabilities.tags, vec!["mix".to_string()]);
        let text = serde_json::to_string(&d).unwrap();
        let back: OperationDescriptor = serde_json::from_str(&text).unwrap();
        assert_eq!(back.executor_kind, ExecutorKind::Nary);
        assert_eq!(back.id.as_str(), "rf.custom.mix");
    }

    #[test]
    fn serde_missing_executor_kind_defaults_unary() {
        let json = r#"{
            "id": "rf.legacy.op",
            "version": { "major": 1, "minor": 0, "patch": 0 },
            "input": { "video": true },
            "output": { "video": true },
            "backend": "rust",
            "deterministic": true
        }"#;
        let d: OperationDescriptor = serde_json::from_str(json).unwrap();
        assert_eq!(d.executor_kind, ExecutorKind::Unary);
        assert_eq!(d.id.as_str(), "rf.legacy.op");
    }

    #[test]
    fn compose_and_mix_stay_nary() {
        let r = OperationRegistry::with_builtins();
        assert_eq!(
            r.get(&OperationId::new("rf.compose.layers"))
                .unwrap()
                .executor_kind,
            ExecutorKind::Nary
        );
        assert_eq!(
            r.get(&OperationId::new("rf.audio.mix"))
                .unwrap()
                .executor_kind,
            ExecutorKind::Nary
        );
    }
}
