//! Bounded pool of RGB24 byte buffers for encode pipelines.

use std::sync::Mutex;

/// Reusable RGB24 frame buffers with a fixed capacity ceiling.
#[derive(Debug)]
pub struct RgbFramePool {
    free: Mutex<Vec<Vec<u8>>>,
    frame_len: usize,
    max_buffers: usize,
}

impl RgbFramePool {
    /// Pool holding up to `max_buffers` buffers of `frame_len` bytes each.
    #[must_use]
    pub fn new(frame_len: usize, max_buffers: usize) -> Self {
        Self {
            free: Mutex::new(Vec::with_capacity(max_buffers.min(32))),
            frame_len,
            max_buffers: max_buffers.clamp(1, 32),
        }
    }

    /// Bytes per RGB24 frame.
    #[must_use]
    pub fn frame_len(&self) -> usize {
        self.frame_len
    }

    /// Take a buffer (allocates when the free list is empty).
    #[must_use]
    pub fn take(&self) -> Vec<u8> {
        let mut free = self.free.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        free.pop().map_or_else(
            || Vec::with_capacity(self.frame_len),
            |mut v| {
                v.clear();
                if v.capacity() < self.frame_len {
                    v.reserve(self.frame_len);
                }
                v
            },
        )
    }

    /// Return a buffer to the pool (drops when over capacity).
    pub fn give(&self, mut buf: Vec<u8>) {
        buf.clear();
        let mut free = self.free.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if free.len() < self.max_buffers {
            free.push(buf);
        }
    }

    /// Number of buffers currently on the free list (test/metrics).
    #[must_use]
    pub fn free_len(&self) -> usize {
        self.free
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuse_buffers() {
        let pool = RgbFramePool::new(12, 2);
        let a = pool.take();
        let b = pool.take();
        pool.give(a);
        pool.give(b);
        assert_eq!(pool.free_len(), 2);
        let c = pool.take();
        assert!(c.capacity() >= 12);
        assert_eq!(pool.free_len(), 1);
    }
}
