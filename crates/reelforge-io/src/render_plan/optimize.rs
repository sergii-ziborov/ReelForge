//! Local rewrite passes over a [`RenderPlan`] op list.

use super::ops::PlanOp;
use super::plan::RenderPlan;

/// Optimization summary (for logs, CLI, benches).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OptimizeStats {
    /// Ops before optimization.
    pub before: usize,
    /// Ops after optimization.
    pub after: usize,
    /// Identity nodes removed.
    pub identities_removed: usize,
    /// Paired flips cancelled.
    pub flips_cancelled: usize,
    /// Consecutive crops merged.
    pub crops_merged: usize,
    /// Consecutive scales merged.
    pub scales_merged: usize,
}

impl OptimizeStats {
    /// How many ops were eliminated.
    #[must_use]
    pub fn eliminated(&self) -> usize {
        self.before.saturating_sub(self.after)
    }
}

/// Result of [`optimize_plan`].
#[derive(Debug, Clone, PartialEq)]
pub struct OptimizedPlan {
    /// Rewritten plan.
    pub plan: RenderPlan,
    /// Pass statistics.
    pub stats: OptimizeStats,
}

/// Run all rewrite passes (pure, no I/O).
///
/// Passes:
/// 1. Drop [`PlanOp::Identity`]
/// 2. Cancel adjacent `HFlip`/`HFlip` and `VFlip`/`VFlip`
/// 3. Merge consecutive crops (second crop in first-crop local coords)
/// 4. Merge consecutive absolute scales (last wins)
#[must_use]
pub fn optimize_plan(plan: &RenderPlan) -> OptimizedPlan {
    let mut stats = OptimizeStats {
        before: plan.ops.len(),
        ..OptimizeStats::default()
    };
    let mut ops = plan.ops.clone();
    stats.identities_removed = remove_identities(&mut ops);
    stats.flips_cancelled = cancel_paired_flips(&mut ops);
    stats.crops_merged = merge_consecutive_crops(&mut ops);
    stats.scales_merged = merge_consecutive_scales(&mut ops);
    // Flips may reappear after merges only if we reorder — we don't; second identity pass is cheap.
    stats.identities_removed += remove_identities(&mut ops);
    stats.after = ops.len();
    let mut out = plan.clone();
    out.ops = ops;
    OptimizedPlan { plan: out, stats }
}

/// Convenience: return only the rewritten plan.
#[must_use]
pub fn optimize(plan: &RenderPlan) -> RenderPlan {
    optimize_plan(plan).plan
}

fn remove_identities(ops: &mut Vec<PlanOp>) -> usize {
    let before = ops.len();
    ops.retain(|op| !matches!(op, PlanOp::Identity));
    before - ops.len()
}

fn cancel_paired_flips(ops: &mut Vec<PlanOp>) -> usize {
    let mut cancelled = 0usize;
    let mut out: Vec<PlanOp> = Vec::with_capacity(ops.len());
    for op in ops.drain(..) {
        match (&op, out.last()) {
            (PlanOp::HFlip, Some(PlanOp::HFlip)) | (PlanOp::VFlip, Some(PlanOp::VFlip)) => {
                out.pop();
                cancelled += 1;
            }
            _ => out.push(op),
        }
    }
    *ops = out;
    cancelled
}

fn merge_consecutive_crops(ops: &mut Vec<PlanOp>) -> usize {
    let mut merged = 0usize;
    let mut out: Vec<PlanOp> = Vec::with_capacity(ops.len());
    for op in ops.drain(..) {
        match (out.last_mut(), &op) {
            (
                Some(PlanOp::Crop {
                    x: x0,
                    y: y0,
                    w: w0,
                    h: h0,
                }),
                PlanOp::Crop {
                    x: x1,
                    y: y1,
                    w: w1,
                    h: h1,
                },
            ) => {
                // Second crop is in the coordinate system of the first crop's output.
                let nx = x0.saturating_add(*x1);
                let ny = y0.saturating_add(*y1);
                if x1.saturating_add(*w1) > *w0 || y1.saturating_add(*h1) > *h0 {
                    // Invalid nested crop — keep separate (validator / runner may error later).
                    out.push(op);
                } else {
                    *x0 = nx;
                    *y0 = ny;
                    *w0 = *w1;
                    *h0 = *h1;
                    merged += 1;
                }
            }
            _ => out.push(op),
        }
    }
    *ops = out;
    merged
}

fn merge_consecutive_scales(ops: &mut Vec<PlanOp>) -> usize {
    let mut merged = 0usize;
    let mut out: Vec<PlanOp> = Vec::with_capacity(ops.len());
    for op in ops.drain(..) {
        match (out.last_mut(), &op) {
            (Some(PlanOp::Scale { w, h }), PlanOp::Scale { w: w1, h: h1 }) => {
                *w = *w1;
                *h = *h1;
                merged += 1;
            }
            _ => out.push(op),
        }
    }
    *ops = out;
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_plan::ops::PlanSource;

    fn plan(ops: Vec<PlanOp>) -> RenderPlan {
        RenderPlan {
            version: 1,
            source: PlanSource::file("in.mp4"),
            ops,
            output: None,
        }
    }

    #[test]
    fn drops_identity() {
        let p = plan(vec![PlanOp::Identity, PlanOp::HFlip, PlanOp::Identity]);
        let o = optimize_plan(&p);
        assert_eq!(o.plan.ops, vec![PlanOp::HFlip]);
        assert_eq!(o.stats.identities_removed, 2);
    }

    #[test]
    fn cancels_double_hflip() {
        let p = plan(vec![PlanOp::HFlip, PlanOp::HFlip, PlanOp::VFlip]);
        let o = optimize_plan(&p);
        assert_eq!(o.plan.ops, vec![PlanOp::VFlip]);
        assert_eq!(o.stats.flips_cancelled, 1);
    }

    #[test]
    fn merges_crops() {
        let p = plan(vec![
            PlanOp::Crop {
                x: 10,
                y: 20,
                w: 200,
                h: 100,
            },
            PlanOp::Crop {
                x: 5,
                y: 5,
                w: 100,
                h: 50,
            },
        ]);
        let o = optimize_plan(&p);
        assert_eq!(
            o.plan.ops,
            vec![PlanOp::Crop {
                x: 15,
                y: 25,
                w: 100,
                h: 50
            }]
        );
        assert_eq!(o.stats.crops_merged, 1);
    }

    #[test]
    fn merges_scales() {
        let p = plan(vec![
            PlanOp::Scale { w: 1280, h: 720 },
            PlanOp::Scale { w: 640, h: 360 },
        ]);
        let o = optimize_plan(&p);
        assert_eq!(o.plan.ops, vec![PlanOp::Scale { w: 640, h: 360 }]);
        assert_eq!(o.stats.scales_merged, 1);
    }
}
