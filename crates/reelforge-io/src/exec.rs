//! Bound executors for compiled [`RenderGraph`](reelforge_render_graph::RenderGraph) ops.
//!
//! Dispatch is [`TypedParams`] / [`ExecutorKind`], not operation-id strings.

use crate::adapter::{AdapterContext, AdapterRequest, execute_adapter};
use crate::error::{IoError, Result};
use crate::gpu::{GpuContext, GpuRequest, execute_gpu};
use crate::graph_run::{GraphEncodeHints, NodeMedia};
use crate::mask_bridge::{apply_region_redaction, region_redaction_from_value};
use reelforge_compose::{
    CompositeLayer, CompositeVideo, MixTrack, composite_video, concatenate_audio,
    concatenate_video, mix_audio,
};
use reelforge_core::{
    AudioClip, AudioEffect, AudioFormat, Duration, MediaTime, Position, Rgb8, SilenceClip, Size,
    Time, VideoClip, VideoEffect, subclip_audio, subclip_video,
};
use reelforge_fx::{
    BlackAndWhite, Crop, CrossFadeIn, EvenSize, FadeIn, FadeOut, Freeze, InvertColors, Loop,
    MirrorX, MirrorY, Painting, Resize, Rotate, SlideIn, SlideOut, SlideSide, Speed, VolumeGain,
};
use reelforge_render_graph::{Animated, CompiledOp, ExecutorKind, TypedParams};
use reelforge_text::{BurnInOptions, burn_in_layers, parse_subtitles_path};
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
        ExecutorKind::Nary => {
            let mut media = execute_nary(compiled, inputs)?;
            if matches!(compiled.params, TypedParams::AudioMix { .. }) && media.audio.is_some() {
                hints.preserve_audio = true;
                media.encode.preserve_audio = Some(true);
            }
            Ok(media)
        }
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
            duration,
        } => {
            let videos: Vec<_> = inputs.iter().map(|m| Arc::clone(&m.video)).collect();
            let audio = inputs.first().and_then(|m| m.audio.clone());
            let video =
                apply_compose_layers(videos, *w, *h, layers, background.as_ref(), *duration)?;
            Ok(NodeMedia {
                video,
                audio,
                masks: inputs.first().and_then(|m| m.masks.clone()),
                encode: inputs.first().map(|m| m.encode.clone()).unwrap_or_default(),
            })
        }
        TypedParams::AudioMix { tracks } => apply_audio_mix(inputs, tracks),
        TypedParams::TimelineConcat { .. } => apply_timeline_concat(inputs),
        other => Err(IoError::message(format!(
            "{} is not an n-ary executor ({other:?})",
            compiled.id
        ))),
    }
}

fn apply_timeline_concat(inputs: Vec<NodeMedia>) -> Result<NodeMedia> {
    if inputs.len() < 2 {
        return Err(IoError::message(
            "rf.timeline.concat needs at least two inputs",
        ));
    }
    let encode = inputs.first().map(|m| m.encode.clone()).unwrap_or_default();
    let mut videos = Vec::with_capacity(inputs.len());
    let mut heard = Vec::with_capacity(inputs.len());
    for media in inputs {
        let picture = media.video.duration();
        if !picture.is_positive() {
            return Err(IoError::message(
                "concat refuses a picture with no positive duration",
            ));
        }
        videos.push(media.video);
        heard.push((picture, media.audio));
    }
    let video = concatenate_video(videos).map_err(|e| IoError::message(e.to_string()))?;
    let Some(format) = heard
        .iter()
        .find_map(|(_, audio)| audio.as_ref().map(AudioClip::format))
    else {
        return Ok(NodeMedia {
            video,
            audio: None,
            masks: None,
            encode,
        });
    };
    let mut pieces: Vec<Arc<dyn AudioClip>> = Vec::with_capacity(heard.len());
    for (picture, audio) in heard {
        pieces.extend(align_concat_audio(format, picture, audio)?);
    }
    let audio = Some(concatenate_audio(pieces).map_err(|e| IoError::message(e.to_string()))?);
    Ok(NodeMedia {
        video,
        audio,
        masks: None,
        encode,
    })
}

/// A missing stream is silence for that picture. A shorter stream is padded.
/// A longer stream is refused, and a different format is refused.
fn align_concat_audio(
    format: AudioFormat,
    picture: Duration,
    audio: Option<Arc<dyn AudioClip>>,
) -> Result<Vec<Arc<dyn AudioClip>>> {
    let picture_frames = format.frames_for_duration(picture);
    if picture_frames == 0 {
        return Err(IoError::message(
            "concat refuses a picture shorter than one audio frame",
        ));
    }
    let Some(clip) = audio else {
        return Ok(vec![silence_frames(format, picture_frames)]);
    };
    if clip.format() != format {
        return Err(IoError::message(format!(
            "concat refuses mixed audio formats: {format:?} and {:?}",
            clip.format()
        )));
    }
    let have = clip.format().frames_for_duration(clip.duration());
    if have > picture_frames {
        return Err(IoError::message(
            "concat refuses audio longer than its picture",
        ));
    }
    let mut pieces = vec![clip];
    if have < picture_frames {
        pieces.push(silence_frames(format, picture_frames - have));
    }
    Ok(pieces)
}

fn silence_frames(format: AudioFormat, frames: u64) -> Arc<dyn AudioClip> {
    Arc::new(SilenceClip::new(format, format.duration_of_frames(frames)))
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
                encode: input.encode,
            })
        }
        TypedParams::AudioDrop => {
            // Drop is node-local. A global flag would mute a later mix or another branch.
            Ok(NodeMedia {
                video: input.video,
                audio: None,
                masks: input.masks,
                encode: input.encode,
            })
        }
        TypedParams::AudioPreserve => {
            hints.preserve_audio = true;
            let mut input = input;
            input.encode.preserve_audio = Some(true);
            Ok(input)
        }
        TypedParams::Trim { start, duration } => apply_trim(input, *start, *duration),
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
                encode: input.encode,
            })
        }
        TypedParams::Gpu {
            name,
            backend,
            params,
        } => execute_gpu_params(name, backend.as_deref(), params, input, hints),
        TypedParams::Speed { factor } => apply_speed(input, *factor),
        TypedParams::EncodeH264 {
            path,
            crf,
            codec,
            fps,
            preserve_audio,
        } => Ok(encode_on_branch(
            input,
            path.as_deref(),
            *crf,
            codec.as_deref(),
            *fps,
            *preserve_audio,
        )),
        TypedParams::ComposeLayers { .. } | TypedParams::AudioMix { .. } => Err(IoError::message(
            format!("{} must be executed as n-ary", compiled.id),
        )),
        other => {
            let video = apply_typed_video(Arc::clone(&input.video), other, hints)?;
            Ok(NodeMedia {
                video,
                audio: input.audio,
                masks: input.masks,
                encode: input.encode,
            })
        }
    }
}

fn apply_audio_mix(inputs: Vec<NodeMedia>, track_value: &serde_json::Value) -> Result<NodeMedia> {
    if inputs.is_empty() {
        return Err(IoError::message("rf.audio.mix needs at least one input"));
    }
    let encode = inputs[0].encode.clone();
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
            if let Some(s) = tp.get("start").and_then(json_as_time) {
                track = track.with_start(s);
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
        encode,
    })
}

#[allow(clippy::many_single_char_names)]
fn apply_compose_layers(
    inputs: Vec<Arc<dyn VideoClip>>,
    w: Option<u32>,
    h: Option<u32>,
    layer_value: &serde_json::Value,
    background: Option<&serde_json::Value>,
    span: Option<MediaTime>,
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
            if lp.get("warp").is_some() {
                return Err(IoError::message(
                    "compose refuses warp; it is not a layer translate",
                ));
            }
            layer = apply_layer_motion(layer, lp)?;
            if let Some(value) = lp.get("opacity") {
                layer = apply_layer_opacity(layer, value)?;
            }
            if let Some(start) = lp.get("start").and_then(json_as_time) {
                layer = layer.with_start(start);
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
        Ok(finish_composite(
            CompositeVideo::with_background(size, color, layers)
                .map_err(|e| IoError::message(e.to_string()))?,
            span,
        ))
    } else {
        Ok(finish_composite(
            CompositeVideo::new(size, layers).map_err(|e| IoError::message(e.to_string()))?,
            span,
        ))
    }
}

fn apply_layer_opacity(layer: CompositeLayer, value: &serde_json::Value) -> Result<CompositeLayer> {
    if let Some(opacity) = value.as_f64() {
        #[allow(clippy::cast_possible_truncation)]
        return Ok(layer.with_opacity(opacity as f32));
    }
    let animated: Animated<f32> = serde_json::from_value(value.clone())
        .map_err(|err| IoError::message(format!("unknown opacity parameter: {err}")))?;
    if let Some(fault) = animated.curve_fault() {
        return Err(IoError::message(format!(
            "compose refuses opacity keyframes: {fault}"
        )));
    }
    Ok(layer.with_opacity_at(Arc::new(move |time: Time| {
        let sampled = match MediaTime::from_secs(time.as_secs(), MediaTime::HZ_1M) {
            Ok(mt) => animated.sample_f32(mt),
            Err(_) => 1.0,
        };
        sampled.clamp(0.0, 1.0)
    })))
}

fn apply_layer_motion(layer: CompositeLayer, lp: &serde_json::Value) -> Result<CompositeLayer> {
    let x = axis_curve(lp.get("x"), "x")?;
    let y = axis_curve(lp.get("y"), "y")?;
    let scale = match lp.get("scale") {
        Some(value) => Some(axis_curve(Some(value), "scale")?),
        None => None,
    };
    let moves = curve_is_animated(&x) || curve_is_animated(&y);
    let mut layer = if moves {
        layer.with_position_at(Arc::new(move |time: Time| {
            (
                pixel_coord(sample_axis(&x, time)),
                pixel_coord(sample_axis(&y, time)),
            )
        }))
    } else {
        layer.with_position(Position::absolute(
            pixel_coord(sample_axis(&x, Time::ZERO)),
            pixel_coord(sample_axis(&y, Time::ZERO)),
        ))
    };
    if let Some(scale) = scale {
        layer = layer.with_scale_at(Arc::new(move |time: Time| sample_axis(&scale, time)));
    }
    Ok(layer)
}

fn curve_is_animated(curve: &Animated<f32>) -> bool {
    matches!(curve, Animated::Keyframes { .. })
}

fn axis_curve(value: Option<&serde_json::Value>, name: &str) -> Result<Animated<f32>> {
    let Some(value) = value else {
        return Ok(Animated::constant(0.0));
    };
    let animated = if let Some(number) = value.as_f64() {
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        Animated::constant(number as f32)
    } else {
        serde_json::from_value(value.clone()).map_err(|err| {
            IoError::message(format!("compose refuses an unreadable {name}: {err}"))
        })?
    };
    if let Some(fault) = animated.curve_fault() {
        return Err(IoError::message(format!(
            "compose refuses {name} keyframes: {fault}"
        )));
    }
    if name == "scale" {
        let values = match &animated {
            Animated::Constant { value } => vec![*value],
            Animated::Keyframes { keys } => keys.iter().map(|key| key.value).collect(),
        };
        if values
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
        {
            return Err(IoError::message("compose refuses a non-positive scale"));
        }
    }
    Ok(animated)
}

fn sample_axis(curve: &Animated<f32>, time: Time) -> f32 {
    let Ok(mt) = MediaTime::from_secs(time.as_secs(), MediaTime::HZ_1M) else {
        return f32::NAN;
    };
    curve.try_sample_f32(mt).unwrap_or(f32::NAN)
}

fn pixel_coord(value: f32) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    let rounded = value.round().clamp(-16_384.0, 16_384.0);
    #[allow(clippy::cast_possible_truncation)]
    {
        rounded as i32
    }
}

fn finish_composite(video: CompositeVideo, span: Option<MediaTime>) -> Arc<dyn VideoClip> {
    let video = match span {
        Some(span) => video.hold_until(span.to_duration()),
        None => video,
    };
    Arc::new(video)
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
        TypedParams::Scale { w, h } => Resize::to_bicubic(Size::new(*w, *h))
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
        TypedParams::CrossFadeIn { duration } => CrossFadeIn::new(duration.to_duration())
            .apply(clip)
            .map_err(IoError::from),
        TypedParams::SlideIn { duration, side } => {
            SlideIn::new(duration.to_duration(), parse_slide_side(side))
                .apply(clip)
                .map_err(IoError::from)
        }
        TypedParams::SlideOut { duration, side } => {
            SlideOut::new(duration.to_duration(), parse_slide_side(side))
                .apply(clip)
                .map_err(IoError::from)
        }
        TypedParams::SubtitleBurn { cues } => apply_subtitle_burn(clip, cues),
        TypedParams::TimelineConcat { .. } => Err(IoError::message(
            "rf.timeline.concat is n-ary — use concatenate_video on gathered inputs",
        )),
        TypedParams::Adapter { .. } | TypedParams::Gpu { .. } => Ok(clip),
        TypedParams::Speed { factor } => {
            VideoEffect::apply(&Speed::new(*factor), clip).map_err(IoError::from)
        }
        TypedParams::Freeze { at, hold } => Freeze::new(at.to_time(), hold.to_duration())
            .apply(clip)
            .map_err(IoError::from),
        TypedParams::Loop { duration, times } => {
            let fx = if let Some(d) = duration {
                Loop::until(d.to_duration())
            } else {
                Loop::times(times.unwrap_or(2))
            };
            fx.apply(clip).map_err(IoError::from)
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

fn apply_trim(input: NodeMedia, start: MediaTime, duration: MediaTime) -> Result<NodeMedia> {
    let video = subclip_video(
        Arc::clone(&input.video),
        start.to_time(),
        duration.to_duration(),
    )
    .map_err(IoError::from)?;
    let audio = match input.audio {
        Some(a) => {
            Some(subclip_audio(a, start.to_time(), duration.to_duration()).map_err(IoError::from)?)
        }
        None => None,
    };
    Ok(NodeMedia {
        video,
        audio,
        masks: input.masks,
        encode: input.encode,
    })
}

fn apply_speed(input: NodeMedia, factor: f64) -> Result<NodeMedia> {
    let video =
        VideoEffect::apply(&Speed::new(factor), Arc::clone(&input.video)).map_err(IoError::from)?;
    let audio = match input.audio {
        Some(a) => Some(AudioEffect::apply(&Speed::new(factor), a).map_err(IoError::from)?),
        None => None,
    };
    Ok(NodeMedia {
        video,
        audio,
        masks: input.masks,
        encode: input.encode,
    })
}

fn encode_on_branch(
    input: NodeMedia,
    path: Option<&str>,
    crf: Option<u8>,
    codec: Option<&str>,
    fps: Option<f64>,
    preserve_audio: Option<bool>,
) -> NodeMedia {
    let mut encode = input.encode;
    if let Some(p) = path {
        encode.path = Some(p.to_owned());
    }
    if let Some(c) = crf {
        encode.crf = Some(c);
    }
    if let Some(name) = codec {
        encode.video_codec = Some(name.to_owned());
    } else {
        encode.video_codec.get_or_insert_with(|| "libx264".into());
    }
    if let Some(f) = fps {
        encode.fps = Some(f);
    }
    if let Some(pa) = preserve_audio {
        encode.preserve_audio = Some(pa);
    }
    NodeMedia {
        video: input.video,
        audio: input.audio,
        masks: input.masks,
        encode,
    }
}

fn execute_gpu_params(
    name: &str,
    backend: Option<&str>,
    params: &serde_json::Value,
    input: NodeMedia,
    hints: &mut GraphEncodeHints,
) -> Result<NodeMedia> {
    let request = GpuRequest::new(
        name.to_string(),
        backend.map(str::to_string),
        params.clone(),
        Arc::clone(&input.video),
    );
    let ctx = GpuContext {
        host: hints.gpu_host.clone(),
        registry: hints.gpu_registry.clone(),
    };
    let out = execute_gpu(&request, &ctx)?;
    let mut encode = input.encode.clone();
    if let Some(codec) = out.video_codec {
        hints.video_codec = Some(codec.clone());
        encode.video_codec = Some(codec);
    }
    Ok(NodeMedia {
        video: out.video.unwrap_or(input.video),
        audio: input.audio,
        masks: input.masks,
        encode,
    })
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

fn json_as_time(v: &serde_json::Value) -> Option<Time> {
    if let Some(s) = v.as_f64() {
        return Some(Time::from_secs(s));
    }
    let ticks = v.get("ticks")?.as_i64()?;
    let timescale = u32::try_from(v.get("timescale")?.as_u64()?).ok()?;
    MediaTime::new(ticks, timescale)
        .ok()
        .map(MediaTime::to_time)
}

fn parse_slide_side(side: &str) -> SlideSide {
    match side {
        "left" => SlideSide::Left,
        "top" => SlideSide::Top,
        "bottom" => SlideSide::Bottom,
        _ => SlideSide::Right,
    }
}

fn apply_subtitle_burn(
    clip: Arc<dyn VideoClip>,
    cues: &serde_json::Value,
) -> Result<Arc<dyn VideoClip>> {
    let Some(arr) = cues.as_array() else {
        return Err(IoError::message("rf.subtitle.burn cues must be an array"));
    };
    if arr.is_empty() {
        return Ok(clip);
    }
    let size = clip.size();
    let mut layers = vec![CompositeLayer::new(Arc::clone(&clip)).with_layer_index(0)];
    for cue in arr {
        let uri = cue
            .get("uri")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| IoError::message("rf.subtitle.burn cue needs uri"))?;
        let record_start = cue
            .get("start")
            .and_then(json_as_time)
            .unwrap_or(Time::ZERO);
        let src_in = cue.get("in").and_then(json_as_time).unwrap_or(Time::ZERO);
        let src_end = cue
            .get("duration")
            .and_then(json_as_time)
            .map(|d| Time::from_secs(src_in.as_secs() + d.as_secs()));
        let parsed = parse_subtitles_path(uri).map_err(|e| IoError::message(e.to_string()))?;
        let mut shifted = Vec::new();
        for mut item in parsed {
            if item.end.as_secs() <= src_in.as_secs() {
                continue;
            }
            if let Some(end) = src_end
                && item.start.as_secs() >= end.as_secs()
            {
                continue;
            }
            let delta = record_start.as_secs() - src_in.as_secs();
            item.start = Time::from_secs(item.start.as_secs() + delta);
            item.end = Time::from_secs(item.end.as_secs() + delta);
            shifted.push(item);
        }
        if shifted.is_empty() {
            continue;
        }
        let burned = burn_in_layers(&shifted, &BurnInOptions::default())
            .map_err(|e| IoError::message(e.to_string()))?;
        layers.extend(burned);
    }
    if layers.len() == 1 {
        return Ok(clip);
    }
    composite_video(size, layers).map_err(|e| IoError::message(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{
        AudioBuffer, AudioClip, AudioFormat, ColorClip, Duration, Rgb8, Size, Time, VideoClip,
    };
    use reelforge_render_graph::Keyframe;

    struct Tone {
        format: AudioFormat,
        duration: Duration,
        value: f32,
    }

    impl AudioClip for Tone {
        fn duration(&self) -> Duration {
            self.duration
        }

        fn format(&self) -> AudioFormat {
            self.format
        }

        fn samples_at(&self, _t: Time, frame_count: usize) -> reelforge_core::Result<AudioBuffer> {
            let channels = usize::from(self.format.channels());
            let samples = vec![self.value; frame_count * channels];
            AudioBuffer::from_interleaved(self.format, samples)
        }
    }

    #[test]
    fn timeline_concat_plays_end_to_end() {
        assert_eq!(
            TypedParams::TimelineConcat {
                clips: serde_json::json!([]),
            }
            .executor_kind(),
            ExecutorKind::Nary
        );
        let white: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(8, 8),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let black: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(8, 8),
            Rgb8::BLACK,
            Duration::from_secs(1.0),
        ));
        let out = apply_timeline_concat(vec![
            NodeMedia::new(white, None),
            NodeMedia::new(black, None),
        ])
        .unwrap();
        assert!((out.video.duration().as_secs() - 2.0).abs() < 1e-6);
        assert_eq!(
            out.video.frame_at(Time::from_secs(0.2)).unwrap().data()[0],
            255
        );
        assert_eq!(
            out.video.frame_at(Time::from_secs(1.2)).unwrap().data()[0],
            0
        );
        assert!(out.audio.is_none());
    }

    #[test]
    fn timeline_concat_keeps_voice_around_a_silent_picture() {
        let fmt = reelforge_core::AudioFormat::STEREO_48K;
        let picture = || {
            Arc::new(ColorClip::new(
                Size::new(2, 2),
                Rgb8::WHITE,
                Duration::from_secs(1.0),
            )) as Arc<dyn VideoClip>
        };
        let voice = |value: f32| -> Arc<dyn reelforge_core::AudioClip> {
            Arc::new(Tone {
                format: fmt,
                duration: Duration::from_secs(1.0),
                value,
            })
        };
        let out = apply_timeline_concat(vec![
            NodeMedia::new(picture(), Some(voice(0.5))),
            NodeMedia::new(picture(), None),
            NodeMedia::new(picture(), Some(voice(-0.25))),
        ])
        .unwrap();
        let audio = out.audio.expect("concat audio");
        assert!((audio.duration().as_secs() - 3.0).abs() < 1e-6);
        let early = audio.samples_at(Time::from_secs(0.1), 4).unwrap();
        assert!(
            early
                .samples()
                .iter()
                .all(|sample| (*sample - 0.5).abs() < 1e-6)
        );
        let middle = audio.samples_at(Time::from_secs(1.2), 4).unwrap();
        assert!(middle.samples().iter().all(|sample| *sample == 0.0));
        let late = audio.samples_at(Time::from_secs(2.1), 4).unwrap();
        assert!(
            late.samples()
                .iter()
                .all(|sample| (*sample + 0.25).abs() < 1e-6)
        );
        assert!((out.video.duration().as_secs() - 3.0).abs() < 1e-6);
    }

    #[test]
    fn timeline_concat_pads_short_audio_and_refuses_a_longer_or_foreign_stream() {
        let fmt = reelforge_core::AudioFormat::STEREO_48K;
        let picture = |secs: f64| {
            Arc::new(ColorClip::new(
                Size::new(2, 2),
                Rgb8::WHITE,
                Duration::from_secs(secs),
            )) as Arc<dyn VideoClip>
        };
        let tone = |secs: f64,
                    format: reelforge_core::AudioFormat|
         -> Arc<dyn reelforge_core::AudioClip> {
            Arc::new(Tone {
                format,
                duration: Duration::from_secs(secs),
                value: 0.5,
            })
        };
        let padded = apply_timeline_concat(vec![
            NodeMedia::new(picture(1.0), Some(tone(0.5, fmt))),
            NodeMedia::new(picture(1.0), Some(tone(1.0, fmt))),
        ])
        .unwrap();
        let audio = padded.audio.expect("padded audio");
        assert!((audio.duration().as_secs() - 2.0).abs() < 1e-6);
        let gap = audio.samples_at(Time::from_secs(0.6), 4).unwrap();
        assert!(gap.samples().iter().all(|sample| *sample == 0.0));
        let second = audio.samples_at(Time::from_secs(1.1), 4).unwrap();
        assert!(
            second
                .samples()
                .iter()
                .all(|sample| (*sample - 0.5).abs() < 1e-6)
        );

        let Err(longer) = apply_timeline_concat(vec![
            NodeMedia::new(picture(1.0), Some(tone(1.5, fmt))),
            NodeMedia::new(picture(1.0), Some(tone(1.0, fmt))),
        ]) else {
            panic!("longer audio was accepted");
        };
        let longer = longer.to_string();
        assert!(longer.contains("longer than its picture"), "{longer}");

        let Err(foreign) = apply_timeline_concat(vec![
            NodeMedia::new(picture(1.0), Some(tone(1.0, fmt))),
            NodeMedia::new(
                picture(1.0),
                Some(tone(1.0, reelforge_core::AudioFormat::STEREO_44K)),
            ),
        ]) else {
            panic!("mixed formats were accepted");
        };
        let foreign = foreign.to_string();
        assert!(foreign.contains("mixed audio formats"), "{foreign}");
    }

    #[test]
    fn crossfade_in_mixes_over_the_opaque_clip() {
        let red: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(1, 1),
            Rgb8::RED,
            Duration::from_secs(2.0),
        ));
        let blue: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(1, 1),
            Rgb8::BLUE,
            Duration::from_secs(2.0),
        ));
        let mut hints = GraphEncodeHints::default();
        let faded = apply_typed_video(
            blue,
            &TypedParams::CrossFadeIn {
                duration: MediaTime::from_secs(0.5, 1_000).unwrap(),
            },
            &mut hints,
        )
        .unwrap();
        let under = CompositeLayer::new(red);
        let over = CompositeLayer::new(faded)
            .with_start(Time::from_secs(1.5))
            .with_layer_index(1);
        let comp = CompositeVideo::new(Size::new(1, 1), vec![under, over]).unwrap();
        let opened = comp.frame_at(Time::from_secs(1.5)).unwrap();
        assert_eq!(&opened.data()[0..3], &[255, 0, 0]);
        let mid = comp.frame_at(Time::from_secs(1.75)).unwrap();
        assert_eq!(&mid.data()[0..3], &[128, 0, 128]);
    }

    #[test]
    fn compose_span_holds_the_background() {
        let layer = CompositeLayer::new(Arc::new(ColorClip::new(
            Size::new(1, 1),
            Rgb8::RED,
            Duration::from_secs(2.0),
        )));
        let video = CompositeVideo::new(Size::new(1, 1), vec![layer]).unwrap();
        let held = finish_composite(video, Some(MediaTime::from_secs(3.0, 1_000).unwrap()));
        assert!((held.duration().as_secs() - 3.0).abs() < 1e-6);
        let tail = held.frame_at(Time::from_secs(2.5)).unwrap();
        assert_eq!(&tail.data()[0..3], &[0, 0, 0]);
    }

    #[test]
    fn opacity_keyframes_change_the_frame_and_seek_repeats() {
        let white = Arc::new(ColorClip::new(
            Size::new(1, 1),
            Rgb8::new(255, 255, 255),
            Duration::from_secs(2.0),
        ));
        let animated = Animated::keyframes(vec![
            Keyframe::new(MediaTime::new(0, 1).unwrap(), 0.0),
            Keyframe::new(MediaTime::new(1, 1).unwrap(), 1.0),
        ]);
        let opacity = serde_json::to_value(&animated).unwrap();
        let layers = serde_json::json!([{ "opacity": opacity }]);
        let clip =
            apply_compose_layers(vec![white], Some(1), Some(1), &layers, None, None).unwrap();
        let sample = |t: f64| clip.frame_at(Time::from_secs(t)).unwrap().data()[0];
        assert_eq!(sample(0.0), 0);
        assert_eq!(sample(1.0), 255);
        let mid = sample(0.5);
        assert!((100..=160).contains(&mid), "mid opacity pixel {mid}");
        assert_eq!(mid, sample(0.5));

        let bad = serde_json::json!([{ "opacity": { "kind": "spline" } }]);
        let Err(err) = apply_compose_layers(
            vec![Arc::new(ColorClip::new(
                Size::new(1, 1),
                Rgb8::new(255, 255, 255),
                Duration::from_secs(1.0),
            ))],
            Some(1),
            Some(1),
            &bad,
            None,
            None,
        ) else {
            panic!("unknown opacity was accepted");
        };
        assert!(err.to_string().contains("unknown opacity"));

        let unsorted = Animated::keyframes(vec![
            Keyframe::new(MediaTime::new(1, 1).unwrap(), 1.0),
            Keyframe::new(MediaTime::new(0, 1).unwrap(), 0.0),
        ]);
        let Err(err) = apply_compose_layers(
            vec![Arc::new(ColorClip::new(
                Size::new(1, 1),
                Rgb8::new(255, 255, 255),
                Duration::from_secs(1.0),
            ))],
            Some(1),
            Some(1),
            &serde_json::json!([{ "opacity": unsorted }]),
            None,
            None,
        ) else {
            panic!("unsorted opacity keyframes were accepted");
        };
        let text = err.to_string();
        assert!(text.contains("refuses"), "{text}");
        assert!(text.contains("not strictly increasing"), "{text}");
    }

    #[test]
    fn position_and_scale_keyframes_change_pixels() {
        let white = Arc::new(ColorClip::new(
            Size::new(1, 1),
            Rgb8::new(255, 255, 255),
            Duration::from_secs(2.0),
        ));
        let x = Animated::keyframes(vec![
            Keyframe::new(MediaTime::new(0, 1).unwrap(), 0.0),
            Keyframe::new(MediaTime::new(1, 1).unwrap(), 2.0),
        ]);
        let moved = apply_compose_layers(
            vec![white],
            Some(3),
            Some(1),
            &serde_json::json!([{ "x": x, "y": 0 }]),
            Some(&serde_json::json!({ "r": 0, "g": 0, "b": 0 })),
            None,
        )
        .unwrap();
        let pixel =
            |t: f64, index: usize| moved.frame_at(Time::from_secs(t)).unwrap().data()[index * 3];
        assert_eq!(pixel(0.0, 0), 255);
        assert_eq!(pixel(0.0, 2), 0);
        assert_eq!(pixel(0.5, 1), 255);
        assert_eq!(pixel(0.5, 0), 0);
        assert_eq!(pixel(1.0, 2), 255);
        assert_eq!(pixel(1.0, 0), 0);

        let block = Arc::new(ColorClip::new(
            Size::new(1, 1),
            Rgb8::new(255, 0, 0),
            Duration::from_secs(2.0),
        ));
        let scale = Animated::keyframes(vec![
            Keyframe::new(MediaTime::new(0, 1).unwrap(), 1.0),
            Keyframe::new(MediaTime::new(1, 1).unwrap(), 2.0),
        ]);
        let grown = apply_compose_layers(
            vec![block],
            Some(2),
            Some(2),
            &serde_json::json!([{ "scale": scale }]),
            Some(&serde_json::json!({ "r": 0, "g": 0, "b": 0 })),
            None,
        )
        .unwrap();
        let red_at = |t: f64, x: usize, y: usize| {
            let frame = grown.frame_at(Time::from_secs(t)).unwrap();
            frame.data()[(y * 2 + x) * 3]
        };
        assert_eq!(red_at(0.0, 0, 0), 255);
        assert_eq!(red_at(0.0, 1, 0), 0);
        assert_eq!(red_at(1.0, 0, 0), 255);
        assert_eq!(red_at(1.0, 1, 1), 255);

        let Err(err) = apply_compose_layers(
            vec![Arc::new(ColorClip::new(
                Size::new(1, 1),
                Rgb8::new(255, 255, 255),
                Duration::from_secs(1.0),
            ))],
            Some(1),
            Some(1),
            &serde_json::json!([{ "warp": { "kind": "keyframes", "keys": [] } }]),
            None,
            None,
        ) else {
            panic!("warp was accepted as a layer translate");
        };
        assert!(err.to_string().contains("refuses warp"), "{err}");
    }
}
