//! CPU-owned context retained by one ESP32-S31 Bluetooth scheduler item.
//!
//! DTM and legacy advertising both bind the same separate `0x48`-byte
//! context from scheduler-item word `+0x04`.  Its contents remain software
//! policy and are deliberately opaque here; this type closes only the stable
//! SRAM allocation and address-lifetime boundary.

#![forbid(unsafe_code)]

/// Bytes reserved for one controller scheduler context.
pub const BLUETOOTH_SCHEDULER_CONTEXT_BYTES: usize = 0x48;

/// Opaque, zero-based CPU-owned scheduler context allocation.
#[repr(C, align(4))]
pub struct SchedulerContextStorage {
    words: [u32; BLUETOOTH_SCHEDULER_CONTEXT_BYTES / 4],
}

impl SchedulerContextStorage {
    pub(crate) const fn new() -> Self {
        Self {
            words: [0; BLUETOOTH_SCHEDULER_CONTEXT_BYTES / 4],
        }
    }

    pub(crate) fn clear(&mut self) {
        self.words.fill(0);
    }
}

impl Default for SchedulerContextStorage {
    fn default() -> Self {
        Self::new()
    }
}
