//! Bounded queue with counted overflow (Sentinel S5: ingestor never blocks the
//! caller beyond a bounded try-push; drops are accounted, never silent).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

/// Bounded MPMC-ingest queue over `std::sync::mpsc::sync_channel` (works from
/// sync and async callers; the runtime worker drains it).
pub struct BoundedQueue<T> {
    tx: mpsc::SyncSender<T>,
    rx: mpsc::Receiver<T>,
    capacity: usize,
    depth: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
}

impl<T> BoundedQueue<T> {
    pub fn new(capacity: usize) -> Self {
        let (tx, rx) = mpsc::sync_channel(capacity.max(1));
        Self {
            tx,
            rx,
            capacity: capacity.max(1),
            depth: Arc::new(AtomicU64::new(0)),
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Non-blocking push. Returns the value back when full so the caller can
    /// apply backpressure policy.
    pub fn try_push(&self, item: T) -> Result<(), T> {
        match self.tx.try_send(item) {
            Ok(()) => {
                self.depth.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(mpsc::TrySendError::Full(item)) | Err(mpsc::TrySendError::Disconnected(item)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                Err(item)
            }
        }
    }

    /// Blocking receive with timeout (worker loop).
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Result<T, mpsc::RecvTimeoutError> {
        let item = self.rx.recv_timeout(timeout)?;
        self.depth.fetch_sub(1, Ordering::Relaxed);
        Ok(item)
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn depth(&self) -> u64 {
        self.depth.load(Ordering::Relaxed)
    }

    /// Cumulative drop count (telemetry_dropped counter source).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_recv_roundtrip_and_depth() {
        let q = BoundedQueue::new(4);
        for i in 0..4 {
            assert!(q.try_push(i).is_ok());
        }
        assert_eq!(q.depth(), 4);
        assert!(q.try_push(99).is_err());
        assert_eq!(q.dropped(), 1);
        assert_eq!(q.recv_timeout(std::time::Duration::from_millis(50)).unwrap(), 0);
        assert_eq!(q.depth(), 3);
    }
}
