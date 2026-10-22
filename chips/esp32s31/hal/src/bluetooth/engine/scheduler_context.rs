//! CPU-owned context retained by one ESP32-S31 Bluetooth scheduler item.
//!
//! DTM and legacy advertising both bind the same separate `0x48`-byte
//! context from scheduler-item word `+0x04`.  Its contents remain software
//! policy and are deliberately opaque here; this type closes only the stable
//! SRAM allocation and address-lifetime boundary.

#![forbid(unsafe_code)]

/// Bytes reserved for one controller scheduler context.
pub const BLUETOOTH_SCHEDULER_CONTEXT_BYTES: usize = crate::layouts::BleSchedulerContext::SIZE;

/// Opaque, zero-based CPU-owned scheduler context allocation.
#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
pub struct SchedulerContextStorage {
    layout: crate::layouts::BleSchedulerContext,
}

impl SchedulerContextStorage {
    pub(crate) const fn new() -> Self {
        Self {
            layout: crate::layouts::BleSchedulerContext::zeroed(),
        }
    }

    pub(crate) fn clear(&mut self) {
        crate::bluetooth::engine::shared_layout::clear_scheduler_context(&self.layout);
    }
}

impl Default for SchedulerContextStorage {
    fn default() -> Self {
        Self::new()
    }
}
