//! Infer and check [`MediaContract`] along a [`CompiledGraph`].
//!
//! Registry descriptors are the *requirement*. Inferred node outputs follow
//! actual media flow (audio passthrough, `audio.drop`, compose first-input
//! audio) so a broken edge fails at compile, not in `graph_run`.

use crate::compile::{CompiledOp, TypedParams};
use crate::compiled::{CompiledNode, CompiledNodeKind};
use crate::error::{GraphError, Result};
use crate::graph::MediaAsset;
use crate::op::{MediaContract, OperationRegistry};

/// Infer the output contract of `node` from already-inferred upstreams.
///
/// `upstreams` must be the output contracts of `node.inputs`, in order.
///
/// # Errors
///
/// [`GraphError::MediaContract`] when a required stream is missing.
pub fn infer_node_contract(
    node: &CompiledNode,
    assets: &[MediaAsset],
    upstreams: &[&MediaContract],
    registry: &OperationRegistry,
) -> Result<MediaContract> {
    match &node.kind {
        CompiledNodeKind::Source { asset } => {
            let meta = assets.get(asset.as_usize()).ok_or_else(|| {
                GraphError::UnknownId(format!("asset index {} on {}", asset, node.id.0))
            })?;
            Ok(source_contract(meta))
        }
        CompiledNodeKind::Output { .. } => {
            let input = single_input(&node.id.0, upstreams)?;
            Ok(input.clone())
        }
        CompiledNodeKind::Redaction { .. } => {
            let input = single_input(&node.id.0, upstreams)?;
            require(
                &node.id.0,
                "rf.redaction.region",
                input,
                &MediaContract::video_only(),
            )?;
            Ok(MediaContract {
                video: true,
                audio: input.audio,
                masks: false,
                notes: None,
            })
        }
        CompiledNodeKind::Op(op) => infer_op_contract(&node.id.0, op, upstreams, registry),
    }
}

fn source_contract(asset: &MediaAsset) -> MediaContract {
    match asset.role.as_deref() {
        Some("audio") => MediaContract::audio_only(),
        Some("mask" | "masks") => MediaContract {
            video: false,
            audio: false,
            masks: true,
            notes: None,
        },
        _ => MediaContract::video_av(),
    }
}

fn infer_op_contract(
    node_id: &str,
    op: &CompiledOp,
    upstreams: &[&MediaContract],
    registry: &OperationRegistry,
) -> Result<MediaContract> {
    let desc = registry.get(&op.id)?;
    match &op.params {
        TypedParams::AudioDrop => {
            let input = single_input(node_id, upstreams)?;
            Ok(MediaContract {
                video: input.video,
                audio: false,
                masks: input.masks,
                notes: None,
            })
        }
        TypedParams::AudioPreserve => {
            let input = single_input(node_id, upstreams)?;
            Ok(input.clone())
        }
        TypedParams::AudioGain { .. } => {
            let input = single_input(node_id, upstreams)?;
            require(node_id, op.id.as_str(), input, &MediaContract::audio_only())?;
            Ok(input.clone())
        }
        TypedParams::AudioMix { .. } => {
            if upstreams.is_empty() {
                return Err(contract_err(node_id, "rf.audio.mix needs inputs"));
            }
            if !upstreams.iter().any(|c| c.audio) {
                return Err(contract_err(
                    node_id,
                    "rf.audio.mix needs at least one audio input",
                ));
            }
            Ok(MediaContract {
                video: upstreams[0].video,
                audio: true,
                masks: false,
                notes: None,
            })
        }
        TypedParams::ComposeLayers { .. } => {
            if upstreams.is_empty() {
                return Err(contract_err(node_id, "rf.compose.layers needs inputs"));
            }
            for (i, c) in upstreams.iter().enumerate() {
                require(
                    node_id,
                    &format!("rf.compose.layers input {i}"),
                    c,
                    &MediaContract::video_only(),
                )?;
            }
            Ok(MediaContract {
                video: true,
                audio: upstreams[0].audio,
                masks: false,
                notes: None,
            })
        }
        TypedParams::EncodeH264 { preserve_audio, .. } => {
            let input = single_input(node_id, upstreams)?;
            require(node_id, op.id.as_str(), input, &MediaContract::video_only())?;
            Ok(MediaContract {
                video: input.video,
                audio: match preserve_audio {
                    Some(false) => false,
                    _ => input.audio,
                },
                masks: false,
                notes: None,
            })
        }
        _ => {
            let input = single_input(node_id, upstreams)?;
            require(node_id, op.id.as_str(), input, &desc.input.without_notes())?;
            // Visual transforms keep companion audio even when the descriptor is video-only.
            Ok(MediaContract {
                video: input.video,
                audio: input.audio,
                masks: false,
                notes: None,
            })
        }
    }
}

fn single_input<'a>(node_id: &str, upstreams: &'a [&MediaContract]) -> Result<&'a MediaContract> {
    match *upstreams {
        [one] => Ok(one),
        [] => Err(contract_err(node_id, "expected exactly one input")),
        _ => Err(contract_err(
            node_id,
            format!("expected exactly one input, got {}", upstreams.len()),
        )),
    }
}

fn require(node_id: &str, what: &str, got: &MediaContract, need: &MediaContract) -> Result<()> {
    if got.satisfies(need) {
        return Ok(());
    }
    Err(contract_err(
        node_id,
        format!(
            "{what} requires video={} audio={} masks={}, got video={} audio={} masks={}",
            need.video, need.audio, need.masks, got.video, got.audio, got.masks
        ),
    ))
}

fn contract_err(node_id: &str, message: impl Into<String>) -> GraphError {
    GraphError::MediaContract(format!("node `{node_id}`: {}", message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiled::{AssetIndex, NodeIndex};
    use crate::graph::{MediaAsset, MediaAssetId, NodeId};
    use crate::op::OperationId;

    fn node(id: &str, kind: CompiledNodeKind, inputs: Vec<NodeIndex>) -> CompiledNode {
        CompiledNode {
            index: NodeIndex(0),
            id: NodeId(id.into()),
            kind,
            inputs,
            output: MediaContract::default(),
        }
    }

    fn asset(role: Option<&str>) -> MediaAsset {
        MediaAsset {
            id: MediaAssetId("a".into()),
            uri: "in.mp4".into(),
            duration: None,
            role: role.map(str::to_string),
        }
    }

    #[test]
    fn source_roles() {
        let n = node(
            "src",
            CompiledNodeKind::Source {
                asset: AssetIndex(0),
            },
            vec![],
        );
        let reg = OperationRegistry::with_builtins();
        let av = infer_node_contract(&n, &[asset(None)], &[], &reg).unwrap();
        assert!(av.video && av.audio);
        let au = infer_node_contract(&n, &[asset(Some("audio"))], &[], &reg).unwrap();
        assert!(!au.video && au.audio);
    }

    #[test]
    fn drop_clears_audio_gain_then_fails() {
        let reg = OperationRegistry::with_builtins();
        let av = MediaContract::video_av();
        let drop = infer_op_contract(
            "drop",
            &crate::compile::compile_op(
                &reg,
                &OperationId::new("rf.audio.drop"),
                &serde_json::json!({}),
            )
            .unwrap(),
            &[&av],
            &reg,
        )
        .unwrap();
        assert!(drop.video && !drop.audio);
        let err = infer_op_contract(
            "gain",
            &crate::compile::compile_op(
                &reg,
                &OperationId::new("rf.audio.gain"),
                &serde_json::json!({ "factor": 0.5 }),
            )
            .unwrap(),
            &[&drop],
            &reg,
        )
        .unwrap_err();
        assert_eq!(err.code_str(), "RFGRAPH_MEDIA_CONTRACT");
    }

    #[test]
    fn color_keeps_companion_audio() {
        let reg = OperationRegistry::with_builtins();
        let av = MediaContract::video_av();
        let out = infer_op_contract(
            "bw",
            &crate::compile::compile_op(
                &reg,
                &OperationId::new("rf.color.black_and_white"),
                &serde_json::json!({}),
            )
            .unwrap(),
            &[&av],
            &reg,
        )
        .unwrap();
        assert!(out.video && out.audio);
    }

    #[test]
    fn color_rejects_audio_only() {
        let reg = OperationRegistry::with_builtins();
        let au = MediaContract::audio_only();
        let err = infer_op_contract(
            "bw",
            &crate::compile::compile_op(
                &reg,
                &OperationId::new("rf.color.black_and_white"),
                &serde_json::json!({}),
            )
            .unwrap(),
            &[&au],
            &reg,
        )
        .unwrap_err();
        assert_eq!(err.code_str(), "RFGRAPH_MEDIA_CONTRACT");
    }
}
