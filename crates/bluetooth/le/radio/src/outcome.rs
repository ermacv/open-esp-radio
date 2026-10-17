//! What a radio event observed: the values a backend reports.

use crate::LeInstant;

/// One PDU received during an event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceivedPdu<'pdu> {
    /// The complete PDU: two-byte header and payload.
    pub pdu: &'pdu [u8],
    /// Receive strength.
    pub rssi_dbm: i8,
    /// The on-air start, or `None` when the hardware captured none.
    pub captured_at: Option<LeInstant>,
}

/// How a scheduled event ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventResult {
    /// The radio executed the event.
    Executed {
        /// The on-air start of the central's first packet of a connection
        /// event, if one arrived.
        anchor: Option<LeInstant>,
    },
    /// The event left the schedule without executing: it was cancelled,
    /// skipped, refused before execution, or its window passed while the
    /// radio was stopped. This is its final terminal, not a retry request.
    NotExecuted,
    /// The radio withdrew the event after publishing it to its hardware,
    /// which left no completion status: the hardware may or may not have
    /// executed it. The hardware no longer reaches the event's memory, so
    /// its resources are returned; it establishes no anchor.
    Aborted,
    /// A fault ended software accounting after hardware may have reached
    /// the event. Execution and completion are unknown. Unlike `Aborted`,
    /// this proves no withdrawal or memory release: hardware owners remain
    /// retained until confirmed teardown, otherwise in quarantine.
    Indeterminate,
}

/// What a Direct Test Mode receiver event returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestReport {
    /// No packet was returned.
    Nothing,
    /// One packet arrived with a valid CRC.
    Received {
        /// Receive strength.
        rssi_dbm: i8,
    },
    /// One packet arrived and failed its check.
    Failed,
}
