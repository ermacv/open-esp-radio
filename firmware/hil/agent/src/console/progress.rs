//! Completion of USB event writes, independently of producer scheduling.

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};

pub struct SerializedEvents {
    next: Watch<CriticalSectionRawMutex, u32, 16>,
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

    pub async fn wait_for(&self, sequence: u32) {
        let target = sequence.wrapping_add(1);
        let mut receiver = self.next.receiver().expect("bounded Wi-Fi event producers");
        // The writer can serialize several events before this producer runs.
        // Sequence order is unambiguous within the bounded in-flight window.
        receiver
            .get_and(|next| next.wrapping_sub(target) < (1 << 31))
            .await;
    }
}
