//! Clip / transition emit (ticks stay as `{ticks, timescale}`).

use crate::emit::CompileCtx;
use crate::error::{ProjectError, Result};
use crate::model::{Retiming, TimelineClip, TransitionKind};
use reelforge_core::MediaTime;
use reelforge_render_graph::{MediaAsset, MediaAssetId, NodeId, RenderNode, RenderNodeKind};
use serde_json::json;

impl CompileCtx<'_> {
    pub(crate) fn emit_clip(&mut self, clip: &TimelineClip) -> Result<(NodeId, MediaTime)> {
        let media = self.lookup_media(&clip.media)?;
        let asset_key = format!("m_{}", media.id.as_str());
        let asset = MediaAsset {
            id: MediaAssetId(asset_key.clone()),
            uri: media.uri.clone(),
            duration: media.duration,
            role: match media.role.as_deref() {
                Some("audio") => None,
                _ => media.role.clone(),
            },
        };
        let asset_id = asset.id.clone();
        self.assets.entry(asset_key).or_insert(asset);
        let src = self.fresh("src");
        self.nodes.push(RenderNode {
            id: src.clone(),
            body: RenderNodeKind::Source { asset: asset_id },
            inputs: Vec::new(),
        });
        let mut node = self.unary(
            "trim",
            "rf.transform.trim",
            json!({
                "start": media_time_json(clip.source.start),
                "duration": media_time_json(clip.source.duration),
            }),
            src,
        );
        node = self.apply_retiming(clip, node);
        if let Some(crop) = clip.crop {
            node = self.unary(
                "crop",
                "rf.transform.crop",
                json!({ "x": crop.x, "y": crop.y, "w": crop.w, "h": crop.h }),
                node,
            );
            if let Some((w, h)) = clip.scale_to {
                node = self.unary(
                    "scale",
                    "rf.transform.scale",
                    json!({ "w": w, "h": h }),
                    node,
                );
            }
        }
        let overlap = self.apply_transition_in(clip, &mut node);
        Ok((node, overlap))
    }

    fn apply_retiming(&mut self, clip: &TimelineClip, node: NodeId) -> NodeId {
        match &clip.retiming {
            Retiming::Identity => node,
            Retiming::Speed { factor } => self.unary(
                "speed",
                "rf.transform.speed",
                json!({ "factor": factor }),
                node,
            ),
            Retiming::Freeze { at, hold } => self.unary(
                "freeze",
                "rf.transform.freeze",
                json!({
                    "at": media_time_json(*at),
                    "hold": media_time_json(*hold),
                }),
                node,
            ),
            Retiming::Loop { duration, times } => {
                let mut params = json!({});
                if let Some(d) = duration {
                    params["duration"] = media_time_json(*d);
                }
                if let Some(n) = times {
                    params["times"] = json!(n);
                }
                self.unary("loop", "rf.transform.loop", params, node)
            }
        }
    }

    fn apply_transition_in(&mut self, clip: &TimelineClip, node: &mut NodeId) -> MediaTime {
        let Some(tr) = &clip.transition_in else {
            return MediaTime::zero(clip.source.duration.timescale.max(1));
        };
        let fade = json!({ "duration": media_time_json(tr.duration) });
        match tr.kind {
            TransitionKind::Fade => {
                *node = self.unary("fin", "rf.transform.fade_in", fade, node.clone());
                MediaTime::zero(tr.duration.timescale.max(1))
            }
            TransitionKind::Dissolve => {
                if let Some(prev) = self.layers.last() {
                    let faded = self.unary(
                        "fout",
                        "rf.transform.fade_out",
                        fade.clone(),
                        prev.node.clone(),
                    );
                    if let Some(last) = self.layers.last_mut() {
                        last.node = faded;
                    }
                }
                *node = self.unary("fin", "rf.transform.fade_in", fade, node.clone());
                tr.duration
            }
            TransitionKind::Wipe => {
                if let Some(prev) = self.layers.last() {
                    let slid = self.unary(
                        "sout",
                        "rf.transform.slide_out",
                        json!({
                            "duration": media_time_json(tr.duration),
                            "side": "left",
                        }),
                        prev.node.clone(),
                    );
                    if let Some(last) = self.layers.last_mut() {
                        last.node = slid;
                    }
                }
                *node = self.unary(
                    "sin",
                    "rf.transform.slide_in",
                    json!({
                        "duration": media_time_json(tr.duration),
                        "side": "right",
                    }),
                    node.clone(),
                );
                self.warnings.push(format!(
                    "clip {}: wipe compiles to opposing slides (LTR push)",
                    clip.id.as_str()
                ));
                tr.duration
            }
        }
    }
}

pub(crate) fn media_time_json(t: MediaTime) -> serde_json::Value {
    json!({ "ticks": t.ticks, "timescale": t.timescale })
}

pub(crate) fn record_duration(clip: &TimelineClip) -> Result<MediaTime> {
    let src = clip.source.duration;
    match clip.retiming {
        Retiming::Identity => Ok(src),
        Retiming::Speed { factor } => src
            .div_f64(factor)
            .map_err(|e| ProjectError::message(format!("clip {}: {e}", clip.id.as_str()))),
        Retiming::Freeze { hold, .. } => src
            .saturating_add(hold)
            .map_err(|e| ProjectError::message(format!("clip {}: {e}", clip.id.as_str()))),
        Retiming::Loop {
            duration: Some(d), ..
        } if d.ticks > 0 => Ok(d),
        Retiming::Loop { times: Some(n), .. } if n > 0 => Ok(src.saturating_mul_u32(n)),
        Retiming::Loop { .. } => Err(ProjectError::message(format!(
            "clip {}: loop needs duration or times",
            clip.id.as_str()
        ))),
    }
}
