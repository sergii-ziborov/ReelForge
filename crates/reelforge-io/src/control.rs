//! Progress reporting and cooperative cancellation for write paths.

use crate::error::{IoError, Result};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Stage of a multi-step write (`write_av` video → audio → mux).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteStage {
    /// Sampling / encoding video frames.
    Video,
    /// Rendering PCM audio.
    Audio,
    /// Muxing video + audio into the final container.
    Mux,
    /// Single-file write finished successfully.
    Done,
}

/// Progress snapshot delivered to [`WriteControl`] callbacks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WriteProgress {
    /// Current stage.
    pub stage: WriteStage,
    /// Zero-based unit index within the stage (frame or audio chunk).
    pub index: u64,
    /// Total units in this stage when known (`0` if unknown).
    pub total: u64,
    /// Approximate completion of this stage in `0.0..=1.0`.
    pub fraction: f64,
}

impl WriteProgress {
    /// Build a progress event.
    #[must_use]
    pub fn new(stage: WriteStage, index: u64, total: u64) -> Self {
        let fraction = if total == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let f = (index as f64 / total as f64).clamp(0.0, 1.0);
            f
        };
        Self {
            stage,
            index,
            total,
            fraction,
        }
    }
}

/// Cooperative cancel flag shared across threads.
#[derive(Clone, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    /// New token (not cancelled).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Whether cancel was requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Error if cancelled.
    ///
    /// # Errors
    ///
    /// Returns [`IoError::Cancelled`].
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(IoError::Cancelled)
        } else {
            Ok(())
        }
    }
}

impl fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// Callback type for write progress.
pub type ProgressCallback = Arc<dyn Fn(WriteProgress) + Send + Sync>;

/// Runtime controls for encode / mux (progress, cancel, pipeline depth).
#[derive(Clone, Default)]
pub struct WriteControl {
    /// Optional cancel token.
    pub cancel: Option<CancelToken>,
    /// Optional progress sink.
    pub on_progress: Option<ProgressCallback>,
    /// Max in-flight sampled frames before ordered encode join.
    ///
    /// `0` or `1` → sequential sample+write. Higher values enable a bounded
    /// worker pipeline (clamped to 32).
    pub max_in_flight: usize,
}

impl WriteControl {
    /// Defaults: sequential, no progress, no cancel.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a cancel token.
    #[must_use]
    pub fn with_cancel(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// Attach a progress callback.
    #[must_use]
    pub fn with_progress<F>(mut self, f: F) -> Self
    where
        F: Fn(WriteProgress) + Send + Sync + 'static,
    {
        self.on_progress = Some(Arc::new(f));
        self
    }

    /// Set pipeline depth (`1` = sequential).
    #[must_use]
    pub fn with_max_in_flight(mut self, n: usize) -> Self {
        self.max_in_flight = n;
        self
    }

    /// Effective in-flight limit (`1..=32`).
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.max_in_flight.clamp(1, 32).max(1)
    }

    /// Check cancel if a token is present.
    ///
    /// # Errors
    ///
    /// Returns [`IoError::Cancelled`].
    pub fn check_cancel(&self) -> Result<()> {
        if let Some(t) = &self.cancel {
            t.check()?;
        }
        Ok(())
    }

    /// Invoke progress callback when set.
    pub fn report(&self, progress: WriteProgress) {
        if let Some(cb) = &self.on_progress {
            cb(progress);
        }
    }
}

impl fmt::Debug for WriteControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteControl")
            .field("cancel", &self.cancel)
            .field("on_progress", &self.on_progress.is_some())
            .field("max_in_flight", &self.max_in_flight)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn cancel_token_trips() {
        let t = CancelToken::new();
        assert!(t.check().is_ok());
        t.cancel();
        assert!(matches!(t.check(), Err(IoError::Cancelled)));
    }

    #[test]
    fn progress_callback_fires() {
        let hits = Arc::new(Mutex::new(0_u32));
        let hits2 = Arc::clone(&hits);
        let c = WriteControl::new().with_progress(move |_| {
            *hits2.lock().unwrap() += 1;
        });
        c.report(WriteProgress::new(WriteStage::Video, 1, 10));
        assert_eq!(*hits.lock().unwrap(), 1);
    }
}
