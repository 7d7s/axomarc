//! Capability action classes guarded by this module (=Policies in D-43 mapping).
pub const READ: &str = "appointment.read";
pub const WRITE: &str = "appointment.write";
pub const TRANSITION: &str = "appointment.transition";
pub const CHECKIN: &str = "appointment.checkin";
pub const CANCEL_APPROVE: &str = "appointment.cancel.approve";
pub const QUEUE_READ: &str = "queue.read";
pub const QUEUE_CALL: &str = "queue.call";

pub const ALL: &[&str] = &[READ, WRITE, TRANSITION, CHECKIN, CANCEL_APPROVE, QUEUE_READ, QUEUE_CALL];