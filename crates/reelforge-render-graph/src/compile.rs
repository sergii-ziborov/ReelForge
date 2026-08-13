//! Compile JSON op params into typed [`CompiledOp`] values.
//!
//! # Architecture
//!
//! | Layer | Responsibility |
//! |-------|----------------|
//! | [`OperationRegistry`] | metadata (backend, version, schema) |
//! | [`compile_op`] | validate params → [`TypedParams`] + [`CostEstimate`] |
//! | `reelforge-io` executors | match on [`TypedParams`] (not free-form JSON) |
//!
//! Call [`check_registry_executor_parity`] in tests so registry and executors
//! cannot silently diverge.

use crate::error::{GraphError, Result};
use crate::op::{BackendClass, OperationId, OperationRegistry, SemVer};
use reelforge_core::MediaTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Relative cost estimate for scheduling (abstract units).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct CostEstimate {
    /// CPU work estimate.
    pub cpu: f64,
    /// Memory / buffer pressure estimate.
    pub memory: f64,
    /// I/O / encode pressure estimate.
    pub io: f64,
    /// GPU work (0 if pure CPU).
    pub gpu: f64,
}

/// Typed, validated parameters for a registered operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TypedParams {
    /// `rf.transform.trim`
    Trim {
        /// Start seconds.
        start: f64,
        /// Duration seconds.
        duration: f64,
    },
    /// `rf.transform.hflip`
    HFlip,
    /// `rf.transform.vflip`
    VFlip,
    /// `rf.transform.even_dims`
    EvenDims,
    /// `rf.transform.scale`
    Scale {
        /// Width.
        w: u32,
        /// Height.
        h: u32,
    },
    /// `rf.transform.crop`
    Crop {
        /// X.
        x: u32,
        /// Y.
        y: u32,
        /// Width.
        w: u32,
        /// Height.
        h: u32,
    },
    /// `rf.transform.rotate`
    Rotate {
        /// Mode string (`cw90`, `degrees`, …).
        mode: String,
        /// Degrees when mode is free-angle.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        degrees: Option<f64>,
    },
    /// `rf.transform.fade_in`
    FadeIn {
        /// Duration seconds.
        duration: f64,
    },
    /// `rf.transform.fade_out`
    FadeOut {
        /// Duration seconds.
        duration: f64,
    },
    /// `rf.color.black_and_white`
    BlackAndWhite,
    /// `rf.color.invert`
    Invert,
    /// `rf.color.painting`
    Painting {
        /// Saturation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        saturation: Option<f32>,
        /// Black/ink.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        black: Option<f32>,
    },
    /// `rf.redaction.region` — full JSON kept after structural checks.
    Redaction {
        /// Validated redaction object (masks non-empty).
        value: Value,
    },
    /// `rf.compose.layers`
    ComposeLayers {
        /// Canvas width.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        w: Option<u32>,
        /// Canvas height.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        h: Option<u32>,
        /// Layer param array passthrough.
        #[serde(default)]
        layers: Value,
        /// Optional background.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        background: Option<Value>,
    },
    /// `rf.audio.gain`
    AudioGain {
        /// Linear factor.
        factor: f32,
    },
    /// `rf.audio.drop`
    AudioDrop,
    /// `rf.audio.preserve`
    AudioPreserve,
    /// `rf.audio.mix`
    AudioMix {
        /// Track params.
        #[serde(default)]
        tracks: Value,
    },
    /// `rf.encode.h264`
    EncodeH264 {
        /// Path override.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// CRF.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crf: Option<u8>,
        /// Codec.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        codec: Option<String>,
        /// FPS.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fps: Option<f64>,
        /// Preserve audio flag.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preserve_audio: Option<bool>,
    },
}

/// Compiled operation ready for stage executors (no free-form JSON field peeking).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledOp {
    /// Registry id.
    pub id: OperationId,
    /// Schema version at compile time.
    pub version: SemVer,
    /// Backend class from registry.
    pub backend: BackendClass,
    /// Typed parameters.
    pub params: TypedParams,
    /// Cost estimate for the scheduler.
    pub cost: CostEstimate,
}

/// Accumulated validation / compile diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompileDiagnostics {
    /// Errors (hard fail).
    pub errors: Vec<GraphError>,
    /// Warnings (non-fatal).
    pub warnings: Vec<String>,
}

impl CompileDiagnostics {
    /// Whether any hard error was recorded.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Push an error.
    pub fn error(&mut self, err: GraphError) {
        self.errors.push(err);
    }

    /// Push a warning.
    pub fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }
}

/// Whether this operation id has a compile/execute path in this crate.
#[must_use]
pub fn is_executable_op_id(id: &str) -> bool {
    matches!(
        id,
        "rf.transform.trim"
            | "rf.transform.hflip"
            | "rf.transform.vflip"
            | "rf.transform.even_dims"
            | "rf.transform.scale"
            | "rf.transform.crop"
            | "rf.transform.rotate"
            | "rf.transform.fade_in"
            | "rf.transform.fade_out"
            | "rf.color.black_and_white"
            | "rf.color.invert"
            | "rf.color.painting"
            | "rf.redaction.region"
            | "rf.compose.layers"
            | "rf.audio.gain"
            | "rf.audio.drop"
            | "rf.audio.preserve"
            | "rf.audio.mix"
            | "rf.encode.h264"
    )
}

/// Validate that every registered builtin used in graphs has an executor.
///
/// # Errors
///
/// Unknown id or registered but not executable.
pub fn ensure_executable(registry: &OperationRegistry, id: &OperationId) -> Result<()> {
    let _ = registry.get(id)?;
    if !is_executable_op_id(id.as_str()) {
        return Err(GraphError::NotExecutable(id.0.clone()));
    }
    Ok(())
}

/// Fail if any `rf.*` registry entry lacks a compile/execute path.
///
/// Use in unit tests to catch registry↔executor drift.
///
/// # Errors
///
/// First non-executable `rf.*` id.
pub fn check_registry_executor_parity(registry: &OperationRegistry) -> Result<()> {
    for id in registry.ids() {
        if id.as_str().starts_with("rf.") {
            ensure_executable(registry, id)?;
        }
    }
    Ok(())
}

/// Compile JSON params for a registered operation into [`CompiledOp`].
///
/// # Errors
///
/// Unknown op, not executable, or invalid params.
pub fn compile_op(
    registry: &OperationRegistry,
    id: &OperationId,
    raw: &Value,
) -> Result<CompiledOp> {
    let desc = registry.get(id)?;
    ensure_executable(registry, id)?;
    let params = parse_typed_params(id.as_str(), raw)?;
    let cost = estimate_cost(id.as_str(), &params);
    Ok(CompiledOp {
        id: id.clone(),
        version: desc.version,
        backend: desc.backend,
        params,
        cost,
    })
}

/// Compile all op nodes in a graph; accumulate diagnostics.
#[must_use]
pub fn compile_graph_ops(
    graph: &crate::graph::RenderGraph,
    registry: &OperationRegistry,
) -> (Vec<(crate::graph::NodeId, CompiledOp)>, CompileDiagnostics) {
    let mut out = Vec::new();
    let mut diag = CompileDiagnostics::default();
    for n in &graph.nodes {
        match &n.body {
            crate::graph::RenderNodeKind::Op { operation, params } => {
                match compile_op(registry, operation, params) {
                    Ok(c) => out.push((n.id.clone(), c)),
                    Err(e) => diag.error(e),
                }
            }
            crate::graph::RenderNodeKind::Redaction { .. } => {
                // Synthetic compile for redaction node.
                match compile_op(
                    registry,
                    &OperationId::new("rf.redaction.region"),
                    &serde_json::json!({}),
                ) {
                    Ok(mut c) => {
                        // Redaction body is on the node, not empty params — mark warning.
                        diag.warn(format!(
                            "node {}: redaction uses node body; op params ignored",
                            n.id.0
                        ));
                        c.params = TypedParams::Redaction {
                            value: serde_json::json!({ "node": n.id.0 }),
                        };
                        out.push((n.id.clone(), c));
                    }
                    Err(e) => diag.error(e),
                }
            }
            _ => {}
        }
    }
    (out, diag)
}

fn f64_field(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| {
        x.as_f64().or_else(|| {
            x.as_i64().map(|i| {
                #[allow(clippy::cast_precision_loss)]
                {
                    i as f64
                }
            })
        })
    })
}

/// Number or `MediaTime` object (`{ticks, timescale}`).
fn time_field(v: &Value, key: &str) -> Option<f64> {
    let x = v.get(key)?;
    if let Some(n) = x.as_f64() {
        return Some(n);
    }
    if let Some(n) = x.as_i64() {
        #[allow(clippy::cast_precision_loss)]
        return Some(n as f64);
    }
    let ticks = x.get("ticks").and_then(serde_json::Value::as_i64)?;
    let ts = x.get("timescale").and_then(serde_json::Value::as_u64)?;
    #[allow(clippy::cast_possible_truncation)]
    let mt = MediaTime::new(ticks, ts as u32).ok()?;
    Some(mt.as_secs())
}

fn u32_field(v: &Value, key: &str) -> Option<u32> {
    v.get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|u| u32::try_from(u).ok())
}

#[allow(clippy::too_many_lines)]
fn parse_typed_params(id: &str, raw: &Value) -> Result<TypedParams> {
    let empty = raw.is_null() || raw.as_object().is_some_and(serde_json::Map::is_empty);
    match id {
        "rf.transform.trim" => {
            let start = time_field(raw, "start").unwrap_or(0.0);
            let duration =
                time_field(raw, "duration").ok_or_else(|| GraphError::InvalidParams {
                    operation: id.into(),
                    message: "duration required".into(),
                })?;
            if !(duration.is_finite() && duration > 0.0) {
                return Err(GraphError::InvalidParams {
                    operation: id.into(),
                    message: "duration must be finite > 0".into(),
                });
            }
            Ok(TypedParams::Trim { start, duration })
        }
        "rf.transform.hflip" => Ok(TypedParams::HFlip),
        "rf.transform.vflip" => Ok(TypedParams::VFlip),
        "rf.transform.even_dims" => Ok(TypedParams::EvenDims),
        "rf.transform.scale" => {
            let w = u32_field(raw, "w").ok_or_else(|| GraphError::InvalidParams {
                operation: id.into(),
                message: "w required".into(),
            })?;
            let h = u32_field(raw, "h").ok_or_else(|| GraphError::InvalidParams {
                operation: id.into(),
                message: "h required".into(),
            })?;
            Ok(TypedParams::Scale { w, h })
        }
        "rf.transform.crop" => {
            let x = u32_field(raw, "x").unwrap_or(0);
            let y = u32_field(raw, "y").unwrap_or(0);
            let w = u32_field(raw, "w").ok_or_else(|| GraphError::InvalidParams {
                operation: id.into(),
                message: "w required".into(),
            })?;
            let h = u32_field(raw, "h").ok_or_else(|| GraphError::InvalidParams {
                operation: id.into(),
                message: "h required".into(),
            })?;
            Ok(TypedParams::Crop { x, y, w, h })
        }
        "rf.transform.rotate" => {
            let mode = raw
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("cw90")
                .to_string();
            let degrees = f64_field(raw, "degrees");
            if mode == "degrees" && degrees.is_none() {
                return Err(GraphError::InvalidParams {
                    operation: id.into(),
                    message: "degrees required when mode=degrees".into(),
                });
            }
            Ok(TypedParams::Rotate { mode, degrees })
        }
        "rf.transform.fade_in" => Ok(TypedParams::FadeIn {
            duration: time_field(raw, "duration").unwrap_or(0.5),
        }),
        "rf.transform.fade_out" => Ok(TypedParams::FadeOut {
            duration: time_field(raw, "duration").unwrap_or(0.5),
        }),
        "rf.color.black_and_white" => Ok(TypedParams::BlackAndWhite),
        "rf.color.invert" => Ok(TypedParams::Invert),
        "rf.color.painting" =>
        {
            #[allow(clippy::cast_possible_truncation)]
            Ok(TypedParams::Painting {
                saturation: f64_field(raw, "saturation").map(|v| v as f32),
                black: f64_field(raw, "black").map(|v| v as f32),
            })
        }
        "rf.redaction.region" => {
            if empty {
                // Allowed when Redaction node carries body.
                return Ok(TypedParams::Redaction {
                    value: serde_json::json!({}),
                });
            }
            if raw.get("masks").is_none() {
                return Err(GraphError::InvalidParams {
                    operation: id.into(),
                    message: "masks required".into(),
                });
            }
            Ok(TypedParams::Redaction { value: raw.clone() })
        }
        "rf.compose.layers" => Ok(TypedParams::ComposeLayers {
            w: u32_field(raw, "w"),
            h: u32_field(raw, "h"),
            layers: raw
                .get("layers")
                .cloned()
                .unwrap_or_else(|| Value::Array(vec![])),
            background: raw.get("background").cloned(),
        }),
        "rf.audio.gain" => {
            #[allow(clippy::cast_possible_truncation)]
            let factor = f64_field(raw, "factor").unwrap_or(1.0) as f32;
            Ok(TypedParams::AudioGain { factor })
        }
        "rf.audio.drop" => Ok(TypedParams::AudioDrop),
        "rf.audio.preserve" => Ok(TypedParams::AudioPreserve),
        "rf.audio.mix" => Ok(TypedParams::AudioMix {
            tracks: raw
                .get("tracks")
                .cloned()
                .unwrap_or_else(|| Value::Array(vec![])),
        }),
        "rf.encode.h264" => {
            #[allow(clippy::cast_possible_truncation)]
            let crf = raw
                .get("crf")
                .and_then(serde_json::Value::as_u64)
                .map(|u| u.min(51) as u8);
            Ok(TypedParams::EncodeH264 {
                path: raw.get("path").and_then(|v| v.as_str()).map(str::to_string),
                crf,
                codec: raw
                    .get("codec")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                fps: f64_field(raw, "fps"),
                preserve_audio: raw
                    .get("preserve_audio")
                    .and_then(serde_json::Value::as_bool),
            })
        }
        other => Err(GraphError::NotExecutable(other.into())),
    }
}

fn estimate_cost(id: &str, params: &TypedParams) -> CostEstimate {
    match (id, params) {
        ("rf.transform.scale" | "rf.transform.crop", _) => CostEstimate {
            cpu: 2.0,
            memory: 2.0,
            io: 0.5,
            gpu: 0.0,
        },
        ("rf.color.painting", _) => CostEstimate {
            cpu: 4.0,
            memory: 2.0,
            io: 0.5,
            gpu: 0.0,
        },
        ("rf.redaction.region", _) => CostEstimate {
            cpu: 5.0,
            memory: 3.0,
            io: 0.5,
            gpu: 0.0,
        },
        ("rf.encode.h264", _) => CostEstimate {
            cpu: 8.0,
            memory: 2.0,
            io: 5.0,
            gpu: 0.0,
        },
        ("rf.compose.layers", _) => CostEstimate {
            cpu: 3.0,
            memory: 4.0,
            io: 0.5,
            gpu: 0.0,
        },
        _ => CostEstimate {
            cpu: 1.0,
            memory: 1.0,
            io: 0.2,
            gpu: 0.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_scale_and_rejects_missing() {
        let r = OperationRegistry::with_builtins();
        let c = compile_op(
            &r,
            &OperationId::new("rf.transform.scale"),
            &serde_json::json!({ "w": 64, "h": 32 }),
        )
        .unwrap();
        assert!(matches!(c.params, TypedParams::Scale { w: 64, h: 32 }));
        assert!(c.cost.cpu > 0.0);

        let err = compile_op(
            &r,
            &OperationId::new("rf.transform.scale"),
            &serde_json::json!({ "w": 1 }),
        )
        .unwrap_err();
        assert_eq!(err.code_str(), "RFGRAPH_INVALID_PARAMS");
    }

    #[test]
    fn not_executable_unknown_custom_id() {
        let mut r = OperationRegistry::with_builtins();
        r.register(crate::op::OperationDescriptor {
            id: OperationId::new("rf.future.thing"),
            version: SemVer::V1,
            input: crate::op::MediaContract::default(),
            output: crate::op::MediaContract::default(),
            backend: BackendClass::Rust,
            deterministic: true,
            capabilities: crate::op::CapabilitySet::default(),
            parameter_schema: Value::Null,
            limits: crate::op::OperationLimits::default(),
        });
        let err = compile_op(&r, &OperationId::new("rf.future.thing"), &Value::Null).unwrap_err();
        assert_eq!(err.code_str(), "RFGRAPH_NOT_EXECUTABLE");
    }

    #[test]
    fn builtins_have_executor_parity() {
        let r = OperationRegistry::with_builtins();
        check_registry_executor_parity(&r).unwrap();
    }

    #[test]
    fn trim_accepts_media_time_duration() {
        let r = OperationRegistry::with_builtins();
        let c = compile_op(
            &r,
            &OperationId::new("rf.transform.trim"),
            &serde_json::json!({
                "start": 0,
                "duration": { "ticks": 1_000_000, "timescale": 1_000_000 }
            }),
        )
        .unwrap();
        match c.params {
            TypedParams::Trim { start, duration } => {
                assert!((start - 0.0).abs() < f64::EPSILON);
                assert!((duration - 1.0).abs() < 1e-9);
            }
            _ => panic!("expected Trim"),
        }
    }
}
