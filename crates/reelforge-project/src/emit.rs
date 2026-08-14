//! Emit timeline items into a [`RenderGraph`].

use crate::error::{ProjectError, Result};
use crate::ids::{MediaRefId, SequenceId};
use crate::model::{SemanticRef, TimelineItem};
use crate::project::{CaptureProject, Sequence, TimelineTrack, TrackKind};
use reelforge_core::MediaTime;
use reelforge_render_graph::{
    MaskTimeline, MediaAsset, NodeId, OperationId, RegionRedaction, RenderNode, RenderNodeKind,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct LayerRef {
    pub node: NodeId,
    pub start: f64,
    pub track: usize,
}

pub(crate) struct AudioRef {
    pub node: NodeId,
    pub start: f64,
}

pub(crate) struct CompileCtx<'a> {
    pub(crate) media: BTreeMap<&'a str, &'a crate::model::MediaRef>,
    sequences: &'a [Sequence],
    stack: BTreeSet<String>,
    pub assets: BTreeMap<String, MediaAsset>,
    pub nodes: Vec<RenderNode>,
    pub layers: Vec<LayerRef>,
    pub audio: Vec<AudioRef>,
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
            warnings,
            next: 0,
        }
    }

    pub(crate) fn emit_sequence(&mut self, seq: &Sequence) -> Result<()> {
        if !self.stack.insert(seq.id.0.clone()) {
            return Err(ProjectError::message(format!(
                "nested sequence cycle at {}",
                seq.id.as_str()
            )));
        }
        for (ti, track) in seq.tracks.iter().enumerate() {
            match track.kind {
                TrackKind::Video if track.muted => self
                    .warnings
                    .push(format!("video track {} is muted", track.id.as_str())),
                TrackKind::Video => self.emit_picture_track(track, ti, false)?,
                TrackKind::Audio if track.muted => self
                    .warnings
                    .push(format!("audio track {} is muted", track.id.as_str())),
                TrackKind::Audio => self.emit_picture_track(track, ti, true)?,
                TrackKind::Subtitle => self.warnings.push(format!(
                    "subtitle track {} is stored but not compiled",
                    track.id.as_str()
                )),
            }
        }
        self.stack.remove(seq.id.0.as_str());
        Ok(())
    }

    fn emit_picture_track(
        &mut self,
        track: &TimelineTrack,
        track_index: usize,
        audio_only: bool,
    ) -> Result<()> {
        let mut cursor = 0.0_f64;
        for item in &track.items {
            match item {
                TimelineItem::Gap(g) => cursor += g.duration.as_secs(),
                TimelineItem::Clip(clip) => {
                    let rec = crate::emit_clip::record_secs(clip)?;
                    let (node, overlap) = self.emit_clip(clip)?;
                    cursor = (cursor - overlap).max(0.0);
                    if audio_only {
                        self.audio.push(AudioRef {
                            node,
                            start: cursor,
                        });
                    } else {
                        self.layers.push(LayerRef {
                            node,
                            start: cursor,
                            track: track_index,
                        });
                    }
                    cursor += rec;
                }
                TimelineItem::Nested(nested) => {
                    let add = nested.duration.map_or_else(
                        || self.lookup_seq(&nested.sequence).map_or(0.0, child_span),
                        MediaTime::as_secs,
                    );
                    let child_id = nested.sequence.clone();
                    let child = self.lookup_seq(&child_id)?.clone();
                    let before_v = self.layers.len();
                    let before_a = self.audio.len();
                    self.emit_sequence(&child)?;
                    for layer in &mut self.layers[before_v..] {
                        layer.start += cursor;
                    }
                    for layer in &mut self.audio[before_a..] {
                        layer.start += cursor;
                    }
                    cursor += add;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn emit_compose(&mut self, canvas: Option<(u32, u32)>) -> NodeId {
        let id = self.fresh("comp");
        let layers: Vec<serde_json::Value> = self
            .layers
            .iter()
            .map(|l| {
                json!({
                    "start": l.start,
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
            tracks.push(json!({ "start": a.start }));
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

    /// Subject/event/query/policy handles → adapter + empty fused redaction.
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
    let mut params = json!({});
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

pub(crate) fn child_span(seq: &Sequence) -> f64 {
    seq.tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video)
        .map(|t| {
            t.items
                .iter()
                .map(|i| match i {
                    TimelineItem::Gap(g) => g.duration.as_secs(),
                    TimelineItem::Clip(c) => crate::emit_clip::record_secs(c).unwrap_or(0.0),
                    TimelineItem::Nested(n) => n.duration.map_or(0.0, MediaTime::as_secs),
                })
                .sum::<f64>()
        })
        .fold(0.0, f64::max)
}
