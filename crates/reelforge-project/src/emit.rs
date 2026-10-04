//! Emit timeline items into a [`RenderGraph`].

/// Timeline cursor as an exact rational second (`num / den`).
///
/// Placing each clip by rebasing onto a 1 kHz clock rounds NTSC durations
/// (`1001/30000`) and drifts by hundreds of milliseconds across a thousand clips.
#[derive(Clone, Copy)]
struct ExactCursor {
    num: i128,
    den: i128,
}

impl ExactCursor {
    const fn zero() -> Self {
        Self { num: 0, den: 1 }
    }

    fn add(&mut self, time: MediaTime) {
        self.shift(time, false);
    }

    fn sub(&mut self, time: MediaTime) {
        self.shift(time, true);
        if self.num < 0 {
            self.num = 0;
        }
    }

    fn offset(self, time: MediaTime) -> MediaTime {
        let mut next = self;
        next.add(time);
        next.to_media()
    }

    fn shift(&mut self, time: MediaTime, subtract: bool) {
        let scale = i128::from(time.timescale.max(1));
        let ticks = i128::from(time.ticks);
        let signed = if subtract { -ticks } else { ticks };
        let num = self
            .num
            .saturating_mul(scale)
            .saturating_add(signed.saturating_mul(self.den));
        let den = self.den.saturating_mul(scale).max(1);
        *self = reduce_rational(num, den);
    }

    fn to_media(self) -> MediaTime {
        if self.den > 0
            && self.den <= i128::from(u32::MAX)
            && (i128::from(i64::MIN)..=i128::from(i64::MAX)).contains(&self.num)
        {
            return MediaTime {
                ticks: i64::try_from(self.num).unwrap_or(0),
                timescale: u32::try_from(self.den).unwrap_or(1),
            };
        }
        let target = 1_000_000_000_i128;
        let num = self.num.saturating_mul(target);
        let half = self.den / 2;
        let adjusted = if num >= 0 { num + half } else { num - half };
        let ticks = if self.den == 0 {
            0
        } else {
            adjusted.div_euclid(self.den)
        };
        let ticks = i64::try_from(ticks).unwrap_or(if ticks.is_positive() {
            i64::MAX
        } else {
            i64::MIN
        });
        MediaTime {
            ticks,
            timescale: 1_000_000_000,
        }
    }
}

fn reduce_rational(num: i128, den: i128) -> ExactCursor {
    if den == 0 {
        return ExactCursor { num: 0, den: 1 };
    }
    let (num, den) = if den < 0 { (-num, -den) } else { (num, den) };
    let g = gcd_i128(num.abs(), den).max(1);
    ExactCursor {
        num: num / g,
        den: den / g,
    }
}

fn gcd_i128(mut a: i128, mut b: i128) -> i128 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

use crate::error::{ProjectError, Result};
use crate::ids::{MediaRefId, SequenceId};
use crate::model::{NestedSequence, SemanticRef, TimelineItem};
use crate::project::{CaptureProject, Sequence, TimelineTrack, TrackKind};
use reelforge_core::MediaTime;
use reelforge_render_graph::{
    MaskTimeline, MediaAsset, NodeId, OperationId, RegionRedaction, RenderNode, RenderNodeKind,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct LayerRef {
    pub node: NodeId,
    pub start: MediaTime,
    pub duration: MediaTime,
    pub track: usize,
}

pub(crate) struct AudioRef {
    pub node: NodeId,
    pub start: MediaTime,
    pub duration: MediaTime,
}

pub(crate) struct SubtitleCueRef {
    pub uri: String,
    pub start: MediaTime,
    pub source_start: MediaTime,
    pub duration: MediaTime,
}

pub(crate) struct CompileCtx<'a> {
    pub(crate) media: BTreeMap<&'a str, &'a crate::model::MediaRef>,
    sequences: &'a [Sequence],
    stack: BTreeSet<String>,
    pub assets: BTreeMap<String, MediaAsset>,
    pub nodes: Vec<RenderNode>,
    pub layers: Vec<LayerRef>,
    pub audio: Vec<AudioRef>,
    pub subtitles: Vec<SubtitleCueRef>,
    pub warnings: Vec<String>,
    next: u32,
}

impl<'a> CompileCtx<'a> {
    pub(crate) fn new(project: &'a CaptureProject, warnings: Vec<String>) -> Self {
        let media = project.media.iter().map(|m| (m.id.as_str(), m)).collect();
        Self {
            media,
            sequences: &project.sequences,
            stack: BTreeSet::new(),
            assets: BTreeMap::new(),
            nodes: Vec::new(),
            layers: Vec::new(),
            audio: Vec::new(),
            subtitles: Vec::new(),
            warnings,
            next: 0,
        }
    }

    pub(crate) fn emit_sequence(&mut self, seq: &Sequence) -> Result<MediaTime> {
        self.emit_sequence_from(seq, 0)
    }

    fn emit_sequence_from(&mut self, seq: &Sequence, track_base: usize) -> Result<MediaTime> {
        if !self.stack.insert(seq.id.0.clone()) {
            return Err(ProjectError::message(format!(
                "nested sequence cycle at {}",
                seq.id.as_str()
            )));
        }
        let mut end = MediaTime::zero(1_000);
        let mut saw_video = false;
        for (ti, track) in seq.tracks.iter().enumerate() {
            let track_index = track_base + ti;
            match track.kind {
                TrackKind::Video if track.muted => self
                    .warnings
                    .push(format!("video track {} is muted", track.id.as_str())),
                TrackKind::Video => {
                    let track_end = self.emit_picture_track(track, track_index, false)?;
                    end = if saw_video {
                        end.max_time(track_end)?
                    } else {
                        track_end
                    };
                    saw_video = true;
                }
                TrackKind::Audio if track.muted => self
                    .warnings
                    .push(format!("audio track {} is muted", track.id.as_str())),
                TrackKind::Audio => {
                    self.emit_picture_track(track, track_index, true)?;
                }
                TrackKind::Subtitle if track.muted => self
                    .warnings
                    .push(format!("subtitle track {} is muted", track.id.as_str())),
                TrackKind::Subtitle => self.emit_subtitle_track(track)?,
            }
        }
        self.stack.remove(seq.id.0.as_str());
        Ok(end)
    }

    fn emit_picture_track(
        &mut self,
        track: &TimelineTrack,
        track_index: usize,
        audio_only: bool,
    ) -> Result<MediaTime> {
        let mut cursor = ExactCursor::zero();
        for item in &track.items {
            match item {
                TimelineItem::Gap(g) => {
                    cursor.add(g.duration);
                }
                TimelineItem::Clip(clip) => {
                    let rec = crate::emit_clip::record_duration(clip)?;
                    let (node, overlap) = self.emit_clip(clip, track_index, !audio_only)?;
                    cursor.sub(overlap);
                    let start = cursor.to_media();
                    if audio_only {
                        self.audio.push(AudioRef {
                            node,
                            start,
                            duration: rec,
                        });
                    } else {
                        self.layers.push(LayerRef {
                            node,
                            start,
                            duration: rec,
                            track: track_index,
                        });
                    }
                    cursor.add(rec);
                }
                TimelineItem::Nested(nested) => {
                    let add = self.place_nested(nested, cursor, Some(track_index))?;
                    cursor.add(add);
                }
            }
        }
        Ok(cursor.to_media())
    }

    fn emit_subtitle_track(&mut self, track: &TimelineTrack) -> Result<()> {
        let mut cursor = ExactCursor::zero();
        for item in &track.items {
            match item {
                TimelineItem::Gap(g) => {
                    cursor.add(g.duration);
                }
                TimelineItem::Clip(clip) => {
                    let rec = crate::emit_clip::record_duration(clip)?;
                    let media = self.lookup_media(&clip.media)?;
                    self.subtitles.push(SubtitleCueRef {
                        uri: media.uri.clone(),
                        start: cursor.to_media(),
                        source_start: clip.source.start,
                        duration: rec,
                    });
                    cursor.add(rec);
                }
                TimelineItem::Nested(nested) => {
                    let add = self.place_nested(nested, cursor, None)?;
                    cursor.add(add);
                }
            }
        }
        Ok(())
    }

    /// Compile `nested` at `cursor` and keep its media inside the declared window.
    ///
    /// Child tracks use a high index while they compile, so a dissolve inside
    /// the child cannot see a parent track. Picture nests then fold onto
    /// `fold_track`.
    fn place_nested(
        &mut self,
        nested: &NestedSequence,
        cursor: ExactCursor,
        fold_track: Option<usize>,
    ) -> Result<MediaTime> {
        let child = self.lookup_seq(&nested.sequence)?.clone();
        let add = match nested.duration {
            Some(d) => d,
            None => child_span(&child)?,
        };
        let before_v = self.layers.len();
        let before_a = self.audio.len();
        let before_s = self.subtitles.len();
        self.emit_sequence_from(&child, 1_000_000 + before_v)?;
        let base = cursor;
        for layer in &mut self.layers[before_v..] {
            layer.start = base.offset(layer.start);
        }
        for layer in &mut self.audio[before_a..] {
            layer.start = base.offset(layer.start);
        }
        for cue in &mut self.subtitles[before_s..] {
            cue.start = base.offset(cue.start);
        }
        if let Some(window) = nested.duration {
            let limit = base.offset(window);
            self.clamp_layers(before_v, limit)?;
            self.clamp_audio(before_a, limit)?;
            self.clamp_cues(before_s, limit)?;
        }
        if let Some(track) = fold_track {
            for layer in &mut self.layers[before_v..] {
                layer.track = track;
            }
        }
        Ok(add)
    }

    fn clamp_layers(&mut self, from: usize, limit: MediaTime) -> Result<()> {
        let mut index = from;
        while index < self.layers.len() {
            let start = self.layers[index].start;
            if cmp_time(start, limit) != std::cmp::Ordering::Less {
                self.layers.remove(index);
                continue;
            }
            let end = start.saturating_add(self.layers[index].duration)?;
            if cmp_time(end, limit) == std::cmp::Ordering::Greater {
                let visible = limit.saturating_sub(start)?;
                if visible.is_zero() {
                    self.layers.remove(index);
                    continue;
                }
                let node = self.layers[index].node.clone();
                let trimmed = self.unary(
                    "hold",
                    "rf.transform.trim",
                    json!({
                        "start": crate::emit_clip::media_time_json(MediaTime::zero(visible.timescale.max(1))),
                        "duration": crate::emit_clip::media_time_json(visible),
                    }),
                    node,
                );
                self.layers[index].node = trimmed;
                self.layers[index].duration = visible;
            }
            index += 1;
        }
        Ok(())
    }

    fn clamp_audio(&mut self, from: usize, limit: MediaTime) -> Result<()> {
        let mut index = from;
        while index < self.audio.len() {
            let start = self.audio[index].start;
            if cmp_time(start, limit) != std::cmp::Ordering::Less {
                self.audio.remove(index);
                continue;
            }
            let end = start.saturating_add(self.audio[index].duration)?;
            if cmp_time(end, limit) == std::cmp::Ordering::Greater {
                let visible = limit.saturating_sub(start)?;
                if visible.is_zero() {
                    self.audio.remove(index);
                    continue;
                }
                let node = self.audio[index].node.clone();
                let trimmed = self.unary(
                    "hold",
                    "rf.transform.trim",
                    json!({
                        "start": crate::emit_clip::media_time_json(MediaTime::zero(visible.timescale.max(1))),
                        "duration": crate::emit_clip::media_time_json(visible),
                    }),
                    node,
                );
                self.audio[index].node = trimmed;
                self.audio[index].duration = visible;
            }
            index += 1;
        }
        Ok(())
    }

    fn clamp_cues(&mut self, from: usize, limit: MediaTime) -> Result<()> {
        let mut index = from;
        while index < self.subtitles.len() {
            let start = self.subtitles[index].start;
            if cmp_time(start, limit) != std::cmp::Ordering::Less {
                self.subtitles.remove(index);
                continue;
            }
            let end = start.saturating_add(self.subtitles[index].duration)?;
            if cmp_time(end, limit) == std::cmp::Ordering::Greater {
                let visible = limit.saturating_sub(start)?;
                if visible.is_zero() {
                    self.subtitles.remove(index);
                    continue;
                }
                self.subtitles[index].duration = visible;
            }
            index += 1;
        }
        Ok(())
    }

    pub(crate) fn emit_compose(
        &mut self,
        canvas: Option<(u32, u32)>,
        span: Option<MediaTime>,
    ) -> NodeId {
        let id = self.fresh("comp");
        let layers: Vec<serde_json::Value> = self
            .layers
            .iter()
            .map(|l| {
                json!({
                    "start": crate::emit_clip::media_time_json(l.start),
                    "layer_index": l.track,
                    "x": 0,
                    "y": 0,
                    "opacity": 1.0,
                })
            })
            .collect();
        let mut params = json!({ "layers": layers });
        if let Some((w, h)) = canvas {
            params["w"] = json!(w);
            params["h"] = json!(h);
        }
        if let Some(span) = span.filter(|span| !span.is_zero()) {
            params["duration"] = crate::emit_clip::media_time_json(span);
        }
        self.nodes.push(RenderNode {
            id: id.clone(),
            body: RenderNodeKind::Op {
                operation: OperationId::new("rf.compose.layers"),
                params,
            },
            inputs: self.layers.iter().map(|l| l.node.clone()).collect(),
        });
        id
    }

    pub(crate) fn emit_audio_mix(&mut self, picture: NodeId) -> NodeId {
        let drop = self.unary("adrop", "rf.audio.drop", json!({}), picture);
        let mix = self.fresh("mix");
        let mut inputs = vec![drop.clone()];
        let mut tracks = vec![json!({})];
        for a in &self.audio {
            inputs.push(a.node.clone());
            tracks.push(json!({ "start": crate::emit_clip::media_time_json(a.start) }));
        }
        self.nodes.push(RenderNode {
            id: mix.clone(),
            body: RenderNodeKind::Op {
                operation: OperationId::new("rf.audio.mix"),
                params: json!({ "tracks": tracks }),
            },
            inputs,
        });
        mix
    }

    /// Subject/event/query/policy handles → adapter.
    /// Empty fused redaction is only attached when a subject or policy is present
    /// (the host fills masks). Query/event-only plans stay adapter-only.
    pub(crate) fn emit_semantic_privacy(
        &mut self,
        refs: &[SemanticRef],
        picture: NodeId,
    ) -> NodeId {
        let adapter = self.unary(
            "vision",
            "rf.adapter.sightloom",
            semantic_adapter_params(refs),
            picture,
        );
        let needs_redaction = refs
            .iter()
            .any(|r| r.kind == "subject" || r.kind == "policy");
        if !needs_redaction {
            return adapter;
        }
        let id = self.fresh("redact");
        self.nodes.push(RenderNode {
            id: id.clone(),
            body: RenderNodeKind::Redaction {
                redaction: RegionRedaction::gaussian(MaskTimeline::new(), 12.0),
            },
            inputs: vec![adapter],
        });
        id
    }

    pub(crate) fn emit_subtitle_burn(&mut self, picture: NodeId) -> NodeId {
        let cues: Vec<serde_json::Value> = self
            .subtitles
            .iter()
            .map(|c| {
                json!({
                    "uri": c.uri,
                    "start": crate::emit_clip::media_time_json(c.start),
                    "in": crate::emit_clip::media_time_json(c.source_start),
                    "duration": crate::emit_clip::media_time_json(c.duration),
                })
            })
            .collect();
        self.unary("subs", "rf.subtitle.burn", json!({ "cues": cues }), picture)
    }

    pub(crate) fn unary(
        &mut self,
        prefix: &str,
        op: &str,
        params: serde_json::Value,
        input: NodeId,
    ) -> NodeId {
        let id = self.fresh(prefix);
        self.nodes.push(RenderNode {
            id: id.clone(),
            body: RenderNodeKind::Op {
                operation: OperationId::new(op),
                params,
            },
            inputs: vec![input],
        });
        id
    }

    pub(crate) fn lookup_media(&self, id: &MediaRefId) -> Result<&crate::model::MediaRef> {
        self.media
            .get(id.as_str())
            .copied()
            .ok_or_else(|| ProjectError::message(format!("unknown media {}", id.as_str())))
    }

    fn lookup_seq(&self, id: &SequenceId) -> Result<&Sequence> {
        self.sequences
            .iter()
            .find(|s| &s.id == id)
            .ok_or_else(|| ProjectError::message(format!("unknown sequence {}", id.as_str())))
    }

    pub(crate) fn fresh(&mut self, prefix: &str) -> NodeId {
        let n = self.next;
        self.next += 1;
        NodeId(format!("n_{prefix}_{n}"))
    }
}

pub(crate) fn semantic_adapter_params(refs: &[SemanticRef]) -> serde_json::Value {
    let listed: Vec<serde_json::Value> = refs
        .iter()
        .map(|r| json!({ "kind": r.kind, "id": r.id }))
        .collect();
    let mut params = json!({ "refs": listed });
    for (key, kind) in [
        ("subjects", "subject"),
        ("events", "event"),
        ("query", "query"),
        ("policy", "policy"),
    ] {
        let ids: Vec<&str> = refs
            .iter()
            .filter(|r| r.kind == kind)
            .map(|r| r.id.as_str())
            .collect();
        if !ids.is_empty() {
            params[key] = json!(ids);
        }
    }
    params
}

pub(crate) fn time_is_after(left: MediaTime, right: MediaTime) -> bool {
    cmp_time(left, right) == std::cmp::Ordering::Greater
}

fn cmp_time(left: MediaTime, right: MediaTime) -> std::cmp::Ordering {
    let lhs = i128::from(left.ticks) * i128::from(right.timescale.max(1));
    let rhs = i128::from(right.ticks) * i128::from(left.timescale.max(1));
    lhs.cmp(&rhs)
}

pub(crate) fn child_span(seq: &Sequence) -> Result<MediaTime> {
    let mut best = MediaTime::zero(1_000);
    for track in &seq.tracks {
        if track.kind != TrackKind::Video {
            continue;
        }
        let mut span = MediaTime::zero(1_000);
        for item in &track.items {
            let add = match item {
                TimelineItem::Gap(g) => g.duration,
                TimelineItem::Clip(c) => crate::emit_clip::record_duration(c)?,
                TimelineItem::Nested(n) => n.duration.unwrap_or_else(|| MediaTime::zero(1_000)),
            };
            span = span.saturating_add(add)?;
        }
        best = best.max_time(span)?;
    }
    Ok(best)
}
