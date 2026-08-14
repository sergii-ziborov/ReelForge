//! Directory-backed [`RenderJob`] store.

use crate::error::{IoError, Result};
use crate::job::{JobId, RenderJob};
use std::fs;
use std::path::{Path, PathBuf};

/// Persist jobs as `{root}/{id}/job.json`.
#[derive(Debug, Clone)]
pub struct JobStore {
    root: PathBuf,
}

impl JobStore {
    /// Create (and ensure) the store directory.
    ///
    /// # Errors
    ///
    /// Cannot create the directory.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)
            .map_err(|e| IoError::message(format!("job store mkdir {}: {e}", root.display())))?;
        Ok(Self { root })
    }

    /// Store root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory for one job.
    #[must_use]
    pub fn job_dir(&self, id: &JobId) -> PathBuf {
        self.root.join(id.as_str())
    }

    /// JSON path for one job.
    #[must_use]
    pub fn job_path(&self, id: &JobId) -> PathBuf {
        self.job_dir(id).join("job.json")
    }

    /// Write `job` (creates the job directory).
    ///
    /// # Errors
    ///
    /// I/O or JSON failures.
    pub fn save(&self, job: &RenderJob) -> Result<()> {
        let dir = self.job_dir(&job.id);
        fs::create_dir_all(&dir)
            .map_err(|e| IoError::message(format!("job mkdir {}: {e}", dir.display())))?;
        let path = self.job_path(&job.id);
        let text = serde_json::to_string_pretty(job)
            .map_err(|e| IoError::message(format!("job serialize: {e}")))?;
        fs::write(&path, text)
            .map_err(|e| IoError::message(format!("job write {}: {e}", path.display())))?;
        Ok(())
    }

    /// Load a job.
    ///
    /// # Errors
    ///
    /// Missing file, I/O, or JSON.
    pub fn load(&self, id: &JobId) -> Result<RenderJob> {
        let path = self.job_path(id);
        let text = fs::read_to_string(&path)
            .map_err(|e| IoError::message(format!("job read {}: {e}", path.display())))?;
        serde_json::from_str(&text)
            .map_err(|e| IoError::message(format!("job parse {}: {e}", path.display())))
    }

    /// List job ids (directory names that contain `job.json`).
    ///
    /// # Errors
    ///
    /// Directory read failures.
    pub fn list(&self) -> Result<Vec<JobId>> {
        let mut ids = Vec::new();
        let rd = fs::read_dir(&self.root)
            .map_err(|e| IoError::message(format!("job list {}: {e}", self.root.display())))?;
        for ent in rd {
            let ent =
                ent.map_err(|e| IoError::message(format!("job list entry: {e}")))?;
            if !ent.path().is_dir() {
                continue;
            }
            let name = ent.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let id = JobId::new(name);
            if self.job_path(&id).is_file() {
                ids.push(id);
            }
        }
        ids.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{JobId, RenderJob};

    #[test]
    fn save_load_list() {
        let dir = tempfile::tempdir().unwrap();
        let store = JobStore::open(dir.path()).unwrap();
        let job = RenderJob::new(JobId::new("job-test-1"));
        store.save(&job).unwrap();
        let back = store.load(&job.id).unwrap();
        assert_eq!(back.id, job.id);
        assert_eq!(store.list().unwrap(), vec![job.id]);
    }
}
