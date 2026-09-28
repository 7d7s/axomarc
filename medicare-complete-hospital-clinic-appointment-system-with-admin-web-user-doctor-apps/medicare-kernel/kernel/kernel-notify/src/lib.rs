//! Hub: in-process fan-out. Every frame is delta-shaped so clients reuse the
//! sync apply path (06 §3). Subject-level StreamFilter enforcement lands M5-06.
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub struct Frame {
    pub channel: String,
    pub stream: String,
    pub seq: i64,
    pub payload: serde_json::Value,
}

#[derive(Clone)]
pub struct Hub {
    tx: broadcast::Sender<Frame>,
}

impl Hub {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn publish(&self, frame: Frame) {
        // No subscribers yet is fine (WS gateway wires in M5).
        let _ = self.tx.send(frame);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Frame> {
        self.tx.subscribe()
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new(4096)
    }
}