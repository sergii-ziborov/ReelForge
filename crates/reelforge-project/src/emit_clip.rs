//! Clip / transition emit (ticks stay as `{ticks, timescale}`).

use crate::emit::CompileCtx;
use crate::error::{ProjectError, Result};
use crate::model::{Retiming, TimelineClip, TransitionKind};
use reelforge_core::MediaTime;
use reelforge_render_graph::{MediaAsset, MediaAssetId, NodeId, RenderNode, RenderNodeKind};
use serde_json::json;

impl CompileCtx<'_> {
    pub(crate) fn emit_clip(&mut self, clip: &TimelineClip) -> Result<(NodeId, f64)> {
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
        if let Retiming::Speed { factor } = clip.retiming {
            node = self.unary(
                "speed",
                "rf.transform.speed",
                json!({ "factor": factor }),
                node,
            );
        }
        let overlap = self.apply_transition_in(clip, &mut node);
        Ok((node, overlap))
    }

    fn apply_transition_in(&mut self, clip: &TimelineClip, node: &mut NodeId) -> f64 {
        let Some(tr) = &clip.transition_in else {
            return 0.0;
        };
        let dur = tr.duration.as_secs();
        let fade = json!({ "duration": media_time_json(tr.duration) });
        match tr.kind {
            TransitionKind::Fade => {
                *node = self.unary("fin", "rf.transform.fade_in", fade, node.clone());
                0.0
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
                dur
            }
            TransitionKind::Wipe => {
                self.warnings.push(format!(
                    "clip {}: wipe is declared but not compiled",
                    clip.id.as_str()
                ));
                0.0
            }
        }
    }
}

pub(crate) fn media_time_json(t: MediaTime) -> serde_json::Value {
    json!({ "ticks": t.ticks, "timescale": t.timescale })
}

pub(crate) fn record_secs(clip: &TimelineClip) -> Result<f64> {
    let src = clip.source.duration.as_secs();
    match clip.retiming {
        Retiming::Identity => Ok(src),
        Retiming::Speed { factor } if factor.is_finite() && factor > 0.0 => Ok(src / factor),
        Retiming::Speed { factor } => Err(ProjectError::message(format!(
            "clip {}: invalid speed factor {factor}",
            clip.id.as_str()
        ))),
    }
}
