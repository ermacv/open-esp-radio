//! Completion of USB event writes, independently of producer scheduling.

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};

const RECEIVERS: usize = 16;

/// An impossible terminal-barrier fan-out. The image retains these observations
/// and resets without performing the action guarded by the failed barrier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceiverLimit {
    pub sequence: u32,
    pub next: Option<u32>,
    pub capacity: usize,
}

pub struct SerializedEvents {
    next: Watch<CriticalSectionRawMutex, u32, RECEIVERS>,
}

impl Default for SerializedEvents {
    fn default() -> Self {
        Self::new()
    }
}

impl SerializedEvents {
    pub const fn new() -> Self {
        Self {
            next: Watch::new_with(0),
        }
    }

    pub fn publish_next(&self, next: u32) {
        self.next.sender().send(next);
    }

    pub async fn wait_for(&self, sequence: u32) -> Result<(), ReceiverLimit> {
        let target = sequence.wrapping_add(1);
        let mut receiver = self.next.receiver().ok_or_else(|| ReceiverLimit {
            sequence,
            next: self.next.try_get(),
            capacity: RECEIVERS,
        })?;
        // The writer can serialize several events before this producer runs.
        // Sequence order is unambiguous within the bounded in-flight window.
        receiver
            .get_and(|next| next.wrapping_sub(target) < (1 << 31))
            .await;
        Ok(())
    }
}
