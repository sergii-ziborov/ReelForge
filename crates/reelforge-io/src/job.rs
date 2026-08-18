//! Durable render job state (engine side; Capture owns queue policy).

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Schema version for persisted [`RenderJob`] files.
pub const RENDER_JOB_VERSION: u32 = 2;

/// Stable job handle.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(pub String);

impl JobId {
    /// Construct from a string.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Time-based id (`job-{millis}-{nanos}`).
    #[must_use]
    pub fn generate() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self(format!(
            "job-{:x}-{:x}",
            now.as_millis(),
            now.subsec_nanos()
        ))
    }

    /// Borrow the raw id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Lifecycle of a render job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Created, not started.
    #[default]
    Queued,
    /// Currently executing.
    Running,
    /// Stopped by cancel; may resume.
    Paused,
    /// Failed; may retry / resume.
    Failed,
    /// Finished successfully.
    Done,
}

impl JobState {
    /// Whether [`crate::run_render_job`] should execute work.
    #[must_use]
    pub const fn is_runnable(self) -> bool {
        matches!(self, Self::Queued | Self::Paused | Self::Failed)
    }
}

/// One persisted stage output (file on disk + fingerprints).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StageArtifactRecord {
    /// [`ExecutionPlan`] stage index that produced this file.
    pub stage_index: u32,
    /// Strong stage fingerprint (inputs + ops + backend + host).
    pub fingerprint: String,
    /// Graph node id this file stands in for.
    pub node_id: String,
    /// Absolute or store-relative path.
    pub uri: String,
    /// SHA-style content hash of `uri` after write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_fingerprint: Option<String>,
}

impl StageArtifactRecord {
    /// Build a record.
    #[must_use]
    pub fn new(
        stage_index: u32,
        fingerprint: impl Into<String>,
        node_id: impl Into<String>,
        uri: impl Into<String>,
    ) -> Self {
        Self {
            stage_index,
            fingerprint: fingerprint.into(),
            node_id: node_id.into(),
            uri: uri.into(),
            file_fingerprint: None,
        }
    }

    /// Attach a file content hash.
    #[must_use]
    pub fn with_file_fingerprint(mut self, hash: impl Into<String>) -> Self {
        self.file_fingerprint = Some(hash.into());
        self
    }
}

/// Progress snapshot after the last completed plan stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct JobCheckpoint {
    /// Next [`ExecutionPlan`](reelforge_render_graph::ExecutionPlan) stage index.
    pub next_stage: u32,
    /// Total stages when known.
    pub total_stages: u32,
    /// Stage fingerprints recorded so far (diagnostics / cache keys).
    #[serde(default)]
    pub stage_fingerprints: Vec<String>,
    /// Validated on-disk products for completed stages.
    #[serde(default)]
    pub stage_artifacts: Vec<StageArtifactRecord>,
}

/// Durable record of one graph render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderJob {
    /// Schema version.
    pub version: u32,
    /// Job id.
    pub id: JobId,
    /// Lifecycle.
    pub state: JobState,
    /// Graph + plan fingerprint (same as full-run cache key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_fingerprint: Option<String>,
    /// Last checkpoint.
    #[serde(default)]
    pub checkpoint: JobCheckpoint,
    /// Final output URI when done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_uri: Option<String>,
    /// Failure message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Created (unix ms).
    pub created_unix_ms: u64,
    /// Last write (unix ms).
    pub updated_unix_ms: u64,
}

impl RenderJob {
    /// New queued job.
    #[must_use]
    pub fn new(id: JobId) -> Self {
        let now = unix_ms();
        Self {
            version: RENDER_JOB_VERSION,
            id,
            state: JobState::Queued,
            run_fingerprint: None,
            checkpoint: JobCheckpoint::default(),
            output_uri: None,
            error: None,
            created_unix_ms: now,
            updated_unix_ms: now,
        }
    }

    /// Attach a run fingerprint.
    #[must_use]
    pub fn with_fingerprint(mut self, fp: impl Into<String>) -> Self {
        self.run_fingerprint = Some(fp.into());
        self
    }

    /// Mark updated now.
    pub fn touch(&mut self) {
        self.updated_unix_ms = unix_ms();
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_and_roundtrip_json() {
        let j = RenderJob::new(JobId::generate()).with_fingerprint("abc");
        let text = serde_json::to_string(&j).unwrap();
        let back: RenderJob = serde_json::from_str(&text).unwrap();
        assert_eq!(back.version, RENDER_JOB_VERSION);
        assert_eq!(back.state, JobState::Queued);
        assert_eq!(back.run_fingerprint.as_deref(), Some("abc"));
        assert!(back.state.is_runnable());
        assert!(!JobState::Done.is_runnable());
    }
}
