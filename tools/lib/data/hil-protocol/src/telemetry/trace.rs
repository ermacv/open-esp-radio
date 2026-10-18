//! Draining the target's reset-retained event trace.
//!
//! The target keeps typed trace entries and snapshot slots in memory that
//! survives resets (`oer-trace`). The host reads the trace's state, sets the
//! recorded channels, and pages out entries and snapshots, including what the
//! previous boot left when it ended in a hang or a panic. Entries travel raw:
//! the host decodes them with the same event types the target records.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// Entries one [`TraceEntries`] page carries.
pub const TRACE_ENTRY_PAGE: usize = 16;
/// Snapshot words one [`TraceSnapshotPage`] carries.
pub const TRACE_SNAPSHOT_PAGE: usize = 64;

/// What the host asks of the trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub enum TraceControl {
    /// Report the trace's state.
    Status,
    /// Discard what the storage holds and record the channels in `mask`.
    Start { mask: u64 },
    /// Record exactly the channels in `mask`, while the trace runs.
    SetMask { mask: u64 },
}

/// The trace's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct TraceStatus {
    /// Whether the image links a trace at all.
    pub installed: bool,
    pub entries: u16,
    pub snapshot_slots: u8,
    pub snapshot_words: u16,
    /// Recording; `false` while it holds the previous boot's trace, or after
    /// a freeze.
    pub running: bool,
    pub frozen: bool,
    pub mask: u64,
    /// The freeze trigger's kind and the first post-trigger entry's tag.
    pub trigger: Option<(u16, u16)>,
    /// The storage holds what the previous boot left: nothing was recorded
    /// over it yet.
    pub holding_previous: bool,
    /// Complete entries and snapshots in storage now.
    pub stored_entries: u16,
    pub stored_snapshots: u8,
}

/// One trace entry, raw; decode its words with the event type of `kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct TraceEntry {
    pub tag: u16,
    pub kind: u16,
    pub t_us: u32,
    pub words: [u32; 2],
}

/// The complete entries of storage slots `first..`, at most one page; a
/// slot without a complete entry is skipped, so `next` names where the next
/// page starts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct TraceEntries {
    pub first: u16,
    pub next: u16,
    pub entries: heapless::Vec<TraceEntry, TRACE_ENTRY_PAGE>,
}

/// Words `offset..` of the snapshot in `slot`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Schema)]
pub struct TraceSnapshotPage {
    pub slot: u8,
    pub point: u16,
    pub tag: u16,
    pub t_us: u32,
    /// The snapshot's word count, and whether more did not fit.
    pub len: u16,
    pub truncated: bool,
    pub offset: u16,
    pub words: heapless::Vec<u32, TRACE_SNAPSHOT_PAGE>,
}
