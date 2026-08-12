//! [`RenderPlan`] document model and JSON I/O.

use super::ops::{PlanOp, PlanOutput, PlanSource, RENDER_PLAN_VERSION};
use crate::error::{IoError, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Typed deterministic render graph (serializable).
///
/// Agents and CLIs exchange this JSON; the optimizer rewrites `ops` and the
/// extractor peels an `FFmpeg` prefix without changing observable geometry when
/// only `FFmpeg`-capable ops are present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderPlan {
    /// Document schema version.
    pub version: u32,
    /// Input media.
    pub source: PlanSource,
    /// Ordered transforms (source → … → output).
    #[serde(default)]
    pub ops: Vec<PlanOp>,
    /// Optional destination (required for [`super::execute::run_render_plan`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<PlanOutput>,
}

impl RenderPlan {
    /// New plan with empty ops.
    #[must_use]
    pub fn new(source: PlanSource) -> Self {
        Self {
            version: RENDER_PLAN_VERSION,
            source,
            ops: Vec::new(),
            output: None,
        }
    }

    /// File source + empty ops.
    #[must_use]
    pub fn from_file(path: impl Into<String>) -> Self {
        Self::new(PlanSource::file(path))
    }

    /// Append an op (builder style).
    #[must_use]
    pub fn then(mut self, op: PlanOp) -> Self {
        self.ops.push(op);
        self
    }

    /// Set output path / encode hints.
    #[must_use]
    pub fn with_output(mut self, output: PlanOutput) -> Self {
        self.output = Some(output);
        self
    }

    /// Number of ops.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// Whether there are no ops.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Serialize to pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns when serde fails (should be rare for this type).
    pub fn to_json_pretty(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| IoError::message(format!("plan json: {e}")))
    }

    /// Serialize to compact JSON.
    ///
    /// # Errors
    ///
    /// Returns when serde fails.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| IoError::message(format!("plan json: {e}")))
    }

    /// Parse JSON text.
    ///
    /// # Errors
    ///
    /// Returns parse errors or unsupported version.
    pub fn from_json(text: &str) -> Result<Self> {
        let plan: Self = serde_json::from_str(text)
            .map_err(|e| IoError::message(format!("parse render plan: {e}")))?;
        plan.validate()?;
        Ok(plan)
    }

    /// Load JSON from a path.
    ///
    /// # Errors
    ///
    /// Returns I/O or parse errors.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .map_err(|e| IoError::message(format!("read plan: {e}")))?;
        Self::from_json(&text)
    }

    /// Write pretty JSON to a path.
    ///
    /// # Errors
    ///
    /// Returns I/O errors.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let text = self.to_json_pretty()?;
        if let Some(parent) = path.as_ref().parent()
            && !parent.as_str_empty()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| IoError::message(format!("create plan dir: {e}")))?;
        }
        std::fs::write(path.as_ref(), text)
            .map_err(|e| IoError::message(format!("write plan: {e}")))
    }

    /// Basic structural checks.
    ///
    /// # Errors
    ///
    /// Unsupported version or empty file path.
    pub fn validate(&self) -> Result<()> {
        if self.version == 0 || self.version > RENDER_PLAN_VERSION {
            return Err(IoError::message(format!(
                "unsupported render plan version {} (max {RENDER_PLAN_VERSION})",
                self.version
            )));
        }
        if let PlanSource::File { path } = &self.source
            && path.trim().is_empty()
        {
            return Err(IoError::message("plan source path is empty"));
        }
        Ok(())
    }
}

trait PathExt {
    fn as_str_empty(&self) -> bool;
}

impl PathExt for Path {
    fn as_str_empty(&self) -> bool {
        self.as_os_str().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_plan::ops::PlanOp;

    #[test]
    fn json_roundtrip() {
        let plan = RenderPlan::from_file("in.mp4")
            .then(PlanOp::Trim {
                start: 1.0,
                duration: 2.0,
            })
            .then(PlanOp::HFlip)
            .then(PlanOp::Scale { w: 320, h: 180 })
            .with_output(PlanOutput::new("out.mp4"));
        let text = plan.to_json_pretty().unwrap();
        let back = RenderPlan::from_json(&text).unwrap();
        assert_eq!(plan, back);
        assert!(
            text.contains("h_flip"),
            "expected snake_case op tag in JSON, got:\n{text}"
        );
    }

    #[test]
    fn rejects_empty_path() {
        let plan = RenderPlan::from_file("   ");
        assert!(plan.validate().is_err());
    }
}
