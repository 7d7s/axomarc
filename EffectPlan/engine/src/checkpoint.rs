//! Resumable planning: the checkpoint record, the digest that ties it to its
//! input, and the validation of state the host hands back.
//!
//! Phase 4 keeps session state in the host, not in the arena: the engine stays
//! the only implementation of the conflict rules, and the arena keeps its size.
//! A checkpoint is therefore a snapshot the host takes of regions the engine
//! already owns — the conflict frontier, the event nodes, the emitted records —
//! plus the counters that say where the run stopped.
//!
//! The engine's job on resume is to refuse. A restored frontier that does not
//! match the input prefix it claims to have come from would produce a graph that
//! is structurally perfect and semantically wrong, which is the worst failure
//! this library can have. So `digest` covers *both* sides: the prefix of the
//! input arrays and the state that prefix is supposed to have produced. The
//! engine writes the record after a run; the host stores it verbatim and pushes
//! it back unchanged; any edit to the prefix, any stale snapshot, and the digest
//! disagrees before a single byte is planned.
//!
//! All arithmetic here is a hash, and a hash is allowed to wrap: that is what
//! makes it cheap and what makes it a commitment rather than a checksum of
//! meaning. Nothing in this module decides a dependency.

use crate::{rd_u32, wr_u32, EDGE_BYTES, MAX_CALLS, MAX_EDGES, MAX_EVENTS, RES_SLOTS, SENTINEL};

/// Revision of the checkpoint record. `0` means "no checkpoint": the engine
/// plans the whole batch from an empty frontier and writes a fresh record.
pub const CHECKPOINT_REVISION: u32 = 1;
/// Eight `u32` words: revision, calls processed, events, edges, digest, and
/// three reserved words that must be zero on both read and write.
pub const CHECKPOINT_RECORD_BYTES: usize = 32;

pub const CP_REVISION: usize = 0;
pub const CP_CALLS: usize = 4;
pub const CP_EVENTS: usize = 8;
pub const CP_EDGES: usize = 12;
pub const CP_DIGEST: usize = 16;
pub const CP_RESERVED: usize = 20;

const FNV_OFFSET: u32 = 0x811c_9dc5;
const FNV_PRIME: u32 = 0x0100_0193;

/// One lane of FNV-1a. Bytes are hashed as their own lane so a byte array and a
/// word array of the same values cannot collide by accident of grouping.
#[inline]
fn mix(hash: u32, lane: u32) -> u32 {
    (hash ^ lane).wrapping_mul(FNV_PRIME)
}

#[inline]
fn fold_bytes(hash: u32, bytes: &[u8]) -> u32 {
    let mut out = mix(hash, bytes.len() as u32);
    let mut chunk = bytes;
    while chunk.len() >= 4 {
        out = mix(out, rd_u32(chunk, 0));
        chunk = &chunk[4..];
    }
    let mut tail = 0u32;
    for (index, byte) in chunk.iter().enumerate() {
        tail |= (*byte as u32) << (index * 8);
    }
    if !chunk.is_empty() {
        out = mix(out, tail);
    }
    out
}

/// The state the host must preserve to resume: the counts alone are enough to
/// locate every byte, because each component is a fixed-stride array.
#[derive(Clone, Copy, Debug)]
pub struct Resume {
    /// Calls already processed. The prefix `[0, calls)` must be unchanged.
    pub calls: usize,
    /// Event nodes already appended.
    pub events: usize,
    /// Records already emitted.
    pub edges: usize,
    /// The digest the engine committed for that state.
    pub digest: u32,
}

/// One word of the record, or `None` when the region is too short.
#[inline]
fn word(record: &[u8], at: usize) -> Option<u32> {
    if record.len() < at.checked_add(4)? {
        return None;
    }
    Some(rd_u32(record, at))
}

/// Reads a pushed record. A revision of `0` is not an error: it means the host
/// asked for a whole-batch run and there is nothing to resume.
pub fn parse(record: &[u8]) -> Option<Option<Resume>> {
    let revision = word(record, CP_REVISION)?;
    if revision == 0 {
        for offset in [CP_CALLS, CP_EVENTS, CP_EDGES, CP_DIGEST] {
            if word(record, offset)? != 0 {
                return None;
            }
        }
        return Some(None);
    }
    if revision != CHECKPOINT_REVISION {
        return None;
    }
    Some(Some(Resume {
        calls: word(record, CP_CALLS)? as usize,
        events: word(record, CP_EVENTS)? as usize,
        edges: word(record, CP_EDGES)? as usize,
        digest: word(record, CP_DIGEST)?,
    }))
}

/// Writes the record for a finished run. The digest is the caller's, because
/// only the caller has every slice in the shape `digest` expects.
pub fn commit(record: &mut [u8], resume: &Resume) -> bool {
    if record.len() < CHECKPOINT_RECORD_BYTES {
        return false;
    }
    let mut ok = wr_u32(record, CP_RESERVED, 0)
        && wr_u32(record, CP_RESERVED + 4, 0)
        && wr_u32(record, CP_RESERVED + 8, 0)
        && wr_u32(record, CP_CALLS, resume.calls as u32)
        && wr_u32(record, CP_EVENTS, resume.events as u32)
        && wr_u32(record, CP_EDGES, resume.edges as u32)
        && wr_u32(record, CP_DIGEST, resume.digest);
    // The revision goes last, as everywhere else in this ABI: it is the commit
    // record. A reader that finds 0 knows nothing was published.
    ok = ok && wr_u32(record, CP_REVISION, CHECKPOINT_REVISION);
    ok
}

/// Clears the record before a run starts, so a failed run cannot leave a
/// resumable-looking commitment behind.
pub fn clear(record: &mut [u8]) {
    if record.len() < CHECKPOINT_RECORD_BYTES {
        return;
    }
    wr_u32(record, CP_REVISION, 0);
    wr_u32(record, CP_CALLS, 0);
    wr_u32(record, CP_EVENTS, 0);
    wr_u32(record, CP_EDGES, 0);
    wr_u32(record, CP_DIGEST, 0);
}

/// Covers the input prefix and the state it produced. `accesses` is the number
/// of access entries belonging to the first `calls` calls.
#[allow(clippy::too_many_arguments)]
pub fn digest(
    call_modes: &[u8],
    offsets: &[u8],
    resource_ids: &[u8],
    access_modes: &[u8],
    last_writer: &[u8],
    reader_head: &[u8],
    event_op: &[u8],
    event_next: &[u8],
    records: &[u8],
    calls: usize,
    accesses: usize,
    events: usize,
    edges: usize,
) -> u32 {
    let mut out = FNV_OFFSET;
    // Lengths first: a prefix and a different prefix of the same bytes must not
    // fold to the same value.
    out = mix(out, calls as u32);
    out = mix(out, accesses as u32);
    out = mix(out, events as u32);
    out = mix(out, edges as u32);
    out = fold_bytes(out, &call_modes[..calls.min(call_modes.len())]);
    let offset_bytes = (calls + 1).saturating_mul(4).min(offsets.len());
    out = fold_bytes(out, &offsets[..offset_bytes]);
    let id_bytes = accesses.saturating_mul(4).min(resource_ids.len());
    out = fold_bytes(out, &resource_ids[..id_bytes]);
    out = fold_bytes(out, &access_modes[..accesses.min(access_modes.len())]);
    // The frontier is full-width: every resource slot can hold state even when
    // the batch touched few resources.
    out = fold_bytes(
        out,
        &last_writer[..RES_SLOTS.saturating_mul(4).min(last_writer.len())],
    );
    out = fold_bytes(
        out,
        &reader_head[..RES_SLOTS.saturating_mul(4).min(reader_head.len())],
    );
    let event_bytes = events.saturating_mul(4).min(event_op.len());
    out = fold_bytes(out, &event_op[..event_bytes]);
    let link_bytes = events.saturating_mul(4).min(event_next.len());
    out = fold_bytes(out, &event_next[..link_bytes]);
    let record_bytes = edges.saturating_mul(EDGE_BYTES).min(records.len());
    out = fold_bytes(out, &records[..record_bytes]);
    out
}

/// Checks a resumed run's parameters before any state is trusted. Every bound
/// here is a ceiling the planner can otherwise walk past: `last_writer` slots
/// name calls, event links name nodes, and a chain that did not decrease would
/// turn the drain in `process_write` into a loop over host-chosen memory.
#[allow(clippy::too_many_arguments)]
pub fn validate(
    resume: &Resume,
    call_modes: &[u8],
    resource_ids: &[u8],
    access_modes: &[u8],
    last_writer: &[u8],
    reader_head: &[u8],
    event_op: &[u8],
    event_next: &[u8],
    records: &[u8],
    call_count: usize,
    resource_count: usize,
    access_count: usize,
) -> i32 {
    if resume.calls > call_count
        || resume.calls > MAX_CALLS
        || resume.events > MAX_EVENTS
        || resume.edges > MAX_EDGES
    {
        return crate::ERR_CHECKPOINT_RANGE;
    }
    // Events are one barrier node per call plus one per read access, so the
    // resumed count cannot exceed what the prefix could have produced.
    let ceiling = resume.calls.saturating_mul(2).saturating_add(access_count);
    if resume.events > ceiling || resume.edges > 2 * resume.events {
        return crate::ERR_CHECKPOINT_RANGE;
    }
    if call_modes.len() < call_count
        || resource_ids.len() < access_count.saturating_mul(4)
        || access_modes.len() < access_count
        || last_writer.len() < RES_SLOTS.saturating_mul(4)
        || reader_head.len() < RES_SLOTS.saturating_mul(4)
        || event_op.len() < resume.events.saturating_mul(4)
        || event_next.len() < resume.events.saturating_mul(4)
        || records.len() < resume.edges.saturating_mul(EDGE_BYTES)
    {
        return crate::ERR_RANGE;
    }

    for slot in 0..resource_count {
        let writer = rd_u32(last_writer, slot.saturating_mul(4));
        if writer != SENTINEL && (writer as usize) >= resume.calls {
            return crate::ERR_CHECKPOINT_INVALID;
        }
        let head = rd_u32(reader_head, slot.saturating_mul(4));
        if head > resume.events as u32 {
            return crate::ERR_CHECKPOINT_INVALID;
        }
    }
    for node in 0..resume.events {
        let owner = rd_u32(event_op, node.saturating_mul(4));
        if (owner as usize) >= resume.calls {
            return crate::ERR_CHECKPOINT_INVALID;
        }
        // A read pushes onto the front of its resource's list, so a node's link
        // always points at an older node. A restored chain that does not
        // decrease is not a chain the planner ever wrote.
        let link = rd_u32(event_next, node.saturating_mul(4));
        if link > node as u32 {
            return crate::ERR_CHECKPOINT_INVALID;
        }
    }
    crate::OK
}

const _: () = assert!(CHECKPOINT_RECORD_BYTES == 32);
const _: () = assert!(CP_RESERVED + 12 == CHECKPOINT_RECORD_BYTES);
