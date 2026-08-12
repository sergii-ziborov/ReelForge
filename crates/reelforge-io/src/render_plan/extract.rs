//! Peel the longest pure-`FFmpeg` prefix from a plan.

use super::ops::PlanOp;
use super::optimize::{OptimizedPlan, optimize_plan};
use super::plan::RenderPlan;
use crate::error::{IoError, Result};
use crate::{FilterGraph, FilterOp};
use serde::{Deserialize, Serialize};

/// Execution split after optimization + extraction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtractedPlan {
    /// Plan after local rewrites (source of truth for remainder).
    pub optimized: RenderPlan,
    /// Maximal `FFmpeg`-capable prefix as filter ops.
    pub ffmpeg_ops: Vec<FilterOp>,
    /// Compiled `-vf` string when `ffmpeg_ops` is non-empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ffmpeg_vf: Option<String>,
    /// Ops that must run after the `FFmpeg` segment (Rust / custom).
    pub remainder: Vec<PlanOp>,
    /// True when the whole optimized graph is `FFmpeg`-only (incl. empty).
    pub fully_ffmpeg: bool,
    /// Number of optimized ops assigned to `FFmpeg`.
    pub ffmpeg_op_count: usize,
    /// Number of remainder ops.
    pub remainder_op_count: usize,
}

impl ExtractedPlan {
    /// Build a [`FilterGraph`] from the extracted prefix.
    #[must_use]
    pub fn filter_graph(&self) -> FilterGraph {
        let mut g = FilterGraph::new();
        for op in &self.ffmpeg_ops {
            g = g.then(op.clone());
        }
        g
    }

    /// Whether there is a non-empty `FFmpeg` segment.
    #[must_use]
    pub fn has_ffmpeg_segment(&self) -> bool {
        !self.ffmpeg_ops.is_empty()
    }

    /// Whether any work remains for the Rust path.
    #[must_use]
    pub fn needs_rust_path(&self) -> bool {
        !self.remainder.is_empty()
    }
}

/// Optimize then extract the longest `FFmpeg` prefix.
#[must_use]
pub fn extract_ffmpeg(plan: &RenderPlan) -> ExtractedPlan {
    extract_from_optimized(optimize_plan(plan))
}

/// Extract without re-running optimization (use after [`optimize_plan`]).
#[must_use]
pub fn extract_from_optimized(optimized: OptimizedPlan) -> ExtractedPlan {
    let OptimizedPlan { plan, .. } = optimized;
    let mut ffmpeg_plan_ops = Vec::new();
    let mut remainder = Vec::new();
    let mut split = false;
    for op in &plan.ops {
        if !split && op.is_ffmpeg_capable() {
            ffmpeg_plan_ops.push(op.clone());
        } else {
            split = true;
            remainder.push(op.clone());
        }
    }
    let ffmpeg_ops: Vec<FilterOp> = ffmpeg_plan_ops
        .iter()
        .filter_map(PlanOp::to_filter_op)
        .collect();
    let ffmpeg_vf = if ffmpeg_ops.is_empty() {
        None
    } else {
        let g = {
            let mut g = FilterGraph::new();
            for op in &ffmpeg_ops {
                g = g.then(op.clone());
            }
            g
        };
        g.to_vf().ok()
    };
    let ffmpeg_op_count = ffmpeg_ops.len();
    let remainder_op_count = remainder.len();
    let fully_ffmpeg = remainder.is_empty();
    ExtractedPlan {
        optimized: plan,
        ffmpeg_ops,
        ffmpeg_vf,
        remainder,
        fully_ffmpeg,
        ffmpeg_op_count,
        remainder_op_count,
    }
}

/// Require a fully `FFmpeg`-extractable plan and return its filter graph.
///
/// # Errors
///
/// Returns when remainder ops exist or the graph is empty.
pub fn require_full_ffmpeg(plan: &RenderPlan) -> Result<FilterGraph> {
    let extracted = extract_ffmpeg(plan);
    if !extracted.fully_ffmpeg {
        return Err(IoError::message(format!(
            "plan is not fully FFmpeg-extractable ({} remainder op(s)); first remainder: {:?}",
            extracted.remainder_op_count,
            extracted.remainder.first()
        )));
    }
    if extracted.ffmpeg_ops.is_empty() {
        return Err(IoError::message(
            "plan has no FFmpeg ops after optimization (empty filtergraph)",
        ));
    }
    Ok(extracted.filter_graph())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_plan::ops::{PlanOp, PlanSource};

    fn plan(ops: Vec<PlanOp>) -> RenderPlan {
        RenderPlan {
            version: 1,
            source: PlanSource::file("in.mp4"),
            ops,
            output: None,
        }
    }

    #[test]
    fn full_prefix() {
        let p = plan(vec![
            PlanOp::Trim {
                start: 0.0,
                duration: 5.0,
            },
            PlanOp::HFlip,
            PlanOp::Scale { w: 640, h: 360 },
            PlanOp::EvenDims,
        ]);
        let e = extract_ffmpeg(&p);
        assert!(e.fully_ffmpeg);
        assert_eq!(e.ffmpeg_op_count, 4);
        assert!(e.remainder.is_empty());
        let vf = e.ffmpeg_vf.unwrap();
        assert!(vf.contains("hflip"));
        assert!(vf.contains("scale=640:360"));
    }

    #[test]
    fn splits_on_custom() {
        let p = plan(vec![
            PlanOp::HFlip,
            PlanOp::Custom {
                name: "head_blur".into(),
                params: None,
            },
            PlanOp::Scale { w: 320, h: 180 },
        ]);
        let e = extract_ffmpeg(&p);
        assert!(!e.fully_ffmpeg);
        assert_eq!(e.ffmpeg_op_count, 1);
        assert_eq!(e.remainder_op_count, 2);
        assert!(matches!(e.remainder[0], PlanOp::Custom { .. }));
        // Scale after custom is NOT pulled into FFmpeg (prefix only).
        assert!(matches!(e.remainder[1], PlanOp::Scale { .. }));
    }

    #[test]
    fn optimize_then_extract_merges() {
        let p = plan(vec![
            PlanOp::Identity,
            PlanOp::Scale { w: 1280, h: 720 },
            PlanOp::Scale { w: 640, h: 360 },
            PlanOp::HFlip,
            PlanOp::HFlip,
        ]);
        let e = extract_ffmpeg(&p);
        assert!(e.fully_ffmpeg);
        assert_eq!(e.ffmpeg_op_count, 1); // only final scale remains
        assert_eq!(e.ffmpeg_ops[0], FilterOp::Scale { w: 640, h: 360 });
    }

    #[test]
    fn require_full_rejects_custom() {
        let p = plan(vec![PlanOp::Custom {
            name: "x".into(),
            params: None,
        }]);
        assert!(require_full_ffmpeg(&p).is_err());
    }
}
