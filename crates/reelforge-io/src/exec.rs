//! Bound executors for compiled [`RenderGraph`](reelforge_render_graph::RenderGraph) ops.
//!
//! Dispatch is [`TypedParams`] / [`ExecutorKind`], not operation-id strings.

use crate::adapter::{AdapterContext, AdapterRequest, execute_adapter};
use crate::error::{IoError, Result};
use crate::graph_run::{GraphEncodeHints, NodeMedia};
use crate::mask_bridge::{apply_region_redaction, region_redaction_from_value};
use reelforge_compose::{
    CompositeLayer, MixTrack, composite_video, composite_video_with_background, mix_audio,
};
use reelforge_core::{
    AudioEffect, Position, Rgb8, Size, Time, VideoClip, VideoEffect, subclip_audio, subclip_video,
};
use reelforge_fx::{
    BlackAndWhite, Crop, EvenSize, FadeIn, FadeOut, InvertColors, MirrorX, MirrorY, Painting,
    Resize, Rotate, Speed, VolumeGain,
};
use reelforge_render_graph::{CompiledOp, ExecutorKind, TypedParams};
use std::sync::Arc;

/// Run a compiled op on gathered inputs.
///
/// # Errors
///
/// Missing inputs, invalid typed params, or effect failures.
pub(crate) fn execute_compiled(
    compiled: &CompiledOp,
    inputs: Vec<NodeMedia>,
    hints: &mut GraphEncodeHints,
) -> Result<NodeMedia> {
    match compiled.params.executor_kind() {
        ExecutorKind::Nary => execute_nary(compiled, inputs),
        ExecutorKind::Unary => {
            let input = expect_unary(inputs, compiled.id.as_str())?;
            execute_unary(compiled, input, hints)
        }
    }
}

fn expect_unary(mut inputs: Vec<NodeMedia>, id: &str) -> Result<NodeMedia> {
    if inputs.len() != 1 {
        return Err(IoError::message(format!(
            "{id} expects 1 input, got {}",
            inputs.len()
        )));
    }
    Ok(inputs.remove(0))
}

fn execute_nary(compiled: &CompiledOp, inputs: Vec<NodeMedia>) -> Result<NodeMedia> {
    match &compiled.params {
        TypedParams::ComposeLayers {
            w,
            h,
            layers,
            background,
        } => {
            let videos: Vec<_> = inputs.iter().map(|m| Arc::clone(&m.video)).collect();
            let audio = inputs.first().and_then(|m| m.audio.clone());
            let video = apply_compose_layers(videos, *w, *h, layers, background.as_ref())?;
            Ok(NodeMedia {
                video,
                audio,
                masks: inputs.first().and_then(|m| m.masks.clone()),
            })
        }
        TypedParams::AudioMix { tracks } => apply_audio_mix(inputs, tracks),
        other => Err(IoError::message(format!(
            "{} is not an n-ary executor ({other:?})",
            compiled.id
        ))),
    }
}

fn execute_unary(
    compiled: &CompiledOp,
    input: NodeMedia,
    hints: &mut GraphEncodeHints,
) -> Result<NodeMedia> {
    match &compiled.params {
        TypedParams::AudioGain { factor } => {
            let audio = match input.audio {
                Some(a) => Some(VolumeGain::new(*factor).apply(a).map_err(IoError::from)?),
                None => None,
            };
            Ok(NodeMedia {
                video: input.video,
                audio,
                masks: input.masks,
            })
        }
        TypedParams::AudioDrop => {
            hints.preserve_audio = false;
            Ok(NodeMedia {
                video: input.video,
                audio: None,
                masks: input.masks,
            })
        }
        TypedParams::AudioPreserve => {
            hints.preserve_audio = true;
            Ok(input)
        }
        TypedParams::Trim { start, duration } => {
            let video = subclip_video(
                Arc::clone(&input.video),
                start.to_time(),
                duration.to_duration(),
            )
            .map_err(IoError::from)?;
            let audio = match input.audio {
                Some(a) => Some(
                    subclip_audio(a, start.to_time(), duration.to_duration()).map_err(IoError::from)?,
                ),
                None => None,
            };
            Ok(NodeMedia {
                video,
                audio,
                masks: input.masks,
            })
        }
        TypedParams::Adapter { name, params } => {
            let request = AdapterRequest::new(name.clone(), params.clone())
                .with_video(Arc::clone(&input.video));
            let ctx = AdapterContext {
                host: hints.adapter_host.clone(),
                registry: hints.adapter_registry.clone(),
            };
            let out = execute_adapter(&request, &ctx)?;
            Ok(NodeMedia {
                video: input.video,
                audio: input.audio,
                masks: out.masks.or(input.masks),
            })
        }
        TypedParams::Speed { factor } => {
            let video = VideoEffect::apply(&Speed::new(*factor), Arc::clone(&input.video))
                .map_err(IoError::from)?;
            let audio = match input.audio {
                Some(a) => {
                    Some(AudioEffect::apply(&Speed::new(*factor), a).map_err(IoError::from)?)
                }
                None => None,
            };
            Ok(NodeMedia {
                video,
                audio,
                masks: input.masks,
            })
        }
        TypedParams::ComposeLayers { .. } | TypedParams::AudioMix { .. } => Err(IoError::message(
            format!("{} must be executed as n-ary", compiled.id),
        )),
        other => {
            let video = apply_typed_video(Arc::clone(&input.video), other, hints)?;
            Ok(NodeMedia {
                video,
                audio: input.audio,
                masks: input.masks,
            })
        }
    }
}

fn apply_audio_mix(inputs: Vec<NodeMedia>, track_value: &serde_json::Value) -> Result<NodeMedia> {
    if inputs.is_empty() {
        return Err(IoError::message("rf.audio.mix needs at least one input"));
    }
    let video = Arc::clone(&inputs[0].video);
    let track_params = track_value.as_array();
    let mut tracks = Vec::new();
    for (i, m) in inputs.into_iter().enumerate() {
        let Some(audio) = m.audio else {
            continue;
        };
        let mut track = MixTrack::new(audio);
        if let Some(arr) = track_params
            && let Some(tp) = arr.get(i)
        {
            if let Some(g) = tp.get("gain").and_then(serde_json::Value::as_f64) {
                #[allow(clippy::cast_possible_truncation)]
                {
                    track = track.with_gain(g as f32);
                }
            }
            if let Some(s) = tp.get("start").and_then(serde_json::Value::as_f64) {
                track = track.with_start(Time::from_secs(s));
            }
        }
        tracks.push(track);
    }
    if tracks.is_empty() {
        return Err(IoError::message(
            "rf.audio.mix: no input carries audio to mix",
        ));
    }
    let mixed = mix_audio(tracks).map_err(|e| IoError::message(e.to_string()))?;
    Ok(NodeMedia {
        video,
        audio: Some(mixed),
        masks: None,
    })
}

#[allow(clippy::many_single_char_names)]
fn apply_compose_layers(
    inputs: Vec<Arc<dyn VideoClip>>,
    w: Option<u32>,
    h: Option<u32>,
    layer_value: &serde_json::Value,
    background: Option<&serde_json::Value>,
) -> Result<Arc<dyn VideoClip>> {
    if inputs.is_empty() {
        return Err(IoError::message("rf.compose.layers needs inputs"));
    }
    let size = if let (Some(w), Some(h)) = (w, h) {
        Size::new(w, h)
    } else {
        inputs[0].size()
    };
    let layer_params = layer_value.as_array();
    let mut layers = Vec::with_capacity(inputs.len());
    for (i, clip) in inputs.into_iter().enumerate() {
        let mut layer =
            CompositeLayer::new(clip).with_layer_index(i32::try_from(i).unwrap_or(i32::MAX));
        if let Some(arr) = layer_params
            && let Some(lp) = arr.get(i)
        {
            let x = lp.get("x").and_then(serde_json::Value::as_i64).unwrap_or(0);
            let y = lp.get("y").and_then(serde_json::Value::as_i64).unwrap_or(0);
            #[allow(clippy::cast_possible_truncation)]
            {
                layer = layer.with_position(Position::absolute(x as i32, y as i32));
            }
            if let Some(op) = lp.get("opacity").and_then(serde_json::Value::as_f64) {
                #[allow(clippy::cast_possible_truncation)]
                {
                    layer = layer.with_opacity(op as f32);
                }
            }
            if let Some(start) = lp.get("start").and_then(serde_json::Value::as_f64) {
                layer = layer.with_start(Time::from_secs(start));
            }
            if let Some(idx) = lp.get("layer_index").and_then(serde_json::Value::as_i64) {
                #[allow(clippy::cast_possible_truncation)]
                {
                    layer = layer.with_layer_index(idx as i32);
                }
            }
        }
        layers.push(layer);
    }
    if let Some(bg) = background {
        let r = bg.get("r").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let g = bg.get("g").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let b = bg.get("b").and_then(serde_json::Value::as_u64).unwrap_or(0);
        #[allow(clippy::cast_possible_truncation)]
        let color = Rgb8::new(r as u8, g as u8, b as u8);
        composite_video_with_background(size, color, layers)
            .map_err(|e| IoError::message(e.to_string()))
    } else {
        composite_video(size, layers).map_err(|e| IoError::message(e.to_string()))
    }
}

#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
fn apply_typed_video(
    clip: Arc<dyn VideoClip>,
    params: &TypedParams,
    hints: &mut GraphEncodeHints,
) -> Result<Arc<dyn VideoClip>> {
    match params {
        TypedParams::Trim { start, duration } => {
            subclip_video(clip, start.to_time(), duration.to_duration()).map_err(IoError::from)
        }
        TypedParams::HFlip => MirrorX.apply(clip).map_err(IoError::from),
        TypedParams::VFlip => MirrorY.apply(clip).map_err(IoError::from),
        TypedParams::EvenDims => EvenSize.apply(clip).map_err(IoError::from),
        TypedParams::Scale { w, h } => Resize::to(Size::new(*w, *h))
            .apply(clip)
            .map_err(IoError::from),
        TypedParams::Crop { x, y, w, h } => {
            Crop::new(*x, *y, *w, *h).apply(clip).map_err(IoError::from)
        }
        TypedParams::Rotate { mode, degrees } => apply_rotate_typed(clip, mode, *degrees),
        TypedParams::FadeIn { duration } => FadeIn::new(duration.to_duration())
            .apply(clip)
            .map_err(IoError::from),
        TypedParams::FadeOut { duration } => FadeOut::new(duration.to_duration())
            .apply(clip)
            .map_err(IoError::from),
        TypedParams::Adapter { .. } => Ok(clip),
        TypedParams::Speed { factor } => {
            VideoEffect::apply(&Speed::new(*factor), clip).map_err(IoError::from)
        }
        TypedParams::BlackAndWhite => BlackAndWhite.apply(clip).map_err(IoError::from),
        TypedParams::Invert => InvertColors.apply(clip).map_err(IoError::from),
        TypedParams::Painting { saturation, black } => {
            let paint = match (saturation, black) {
                (Some(s), Some(b)) => Painting::with(*s, *b),
                (Some(s), None) => Painting {
                    saturation: *s,
                    ..Painting::new()
                },
                (None, Some(b)) => Painting {
                    black: *b,
                    ..Painting::new()
                },
                (None, None) => Painting::new(),
            };
            paint.apply(clip).map_err(IoError::from)
        }
        TypedParams::Redaction { value } => {
            let empty = value.is_null() || value.as_object().is_some_and(serde_json::Map::is_empty);
            if empty {
                return Err(IoError::message(
                    "rf.redaction.region requires masks params (or use Redaction node)",
                ));
            }
            let redaction = region_redaction_from_value(value)?;
            apply_region_redaction(clip, &redaction)
        }
        TypedParams::ComposeLayers { .. } | TypedParams::AudioMix { .. } => {
            Err(IoError::message("n-ary op reached unary video path"))
        }
        TypedParams::AudioGain { .. } | TypedParams::AudioDrop | TypedParams::AudioPreserve => {
            Ok(clip)
        }
        TypedParams::EncodeH264 {
            path,
            crf,
            codec,
            fps,
            preserve_audio,
        } => {
            if let Some(p) = path {
                hints.output_path = Some(p.clone());
            }
            if let Some(c) = crf {
                hints.crf = Some(*c);
            }
            if let Some(c) = codec {
                hints.video_codec = Some(c.clone());
            } else {
                hints.video_codec.get_or_insert_with(|| "libx264".into());
            }
            if let Some(f) = fps {
                hints.fps = Some(*f);
            }
            if let Some(pa) = preserve_audio {
                hints.preserve_audio = *pa;
            }
            Ok(clip)
        }
    }
}

fn apply_rotate_typed(
    clip: Arc<dyn VideoClip>,
    mode: &str,
    degrees: Option<f64>,
) -> Result<Arc<dyn VideoClip>> {
    let rot = match mode {
        "cw90" | "90" => Rotate::cw90(),
        "cw180" | "180" => Rotate::half(),
        "cw270" | "270" | "ccw90" => Rotate::cw270(),
        "degrees" => {
            let d = degrees.ok_or_else(|| IoError::message("rotate mode=degrees needs degrees"))?;
            #[allow(clippy::cast_possible_truncation)]
            Rotate::degrees(d as f32)
        }
        other => {
            return Err(IoError::message(format!(
                "unknown rotate mode '{other}' (cw90|cw180|cw270|degrees)"
            )));
        }
    };
    rot.apply(clip).map_err(IoError::from)
}
