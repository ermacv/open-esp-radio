//! Exact-match legacy advertising duplicate history.

use oer_bluetooth_le_wire::{
    LeDeviceAddress,
    scanning::{LEGACY_SCAN_DATA_CAPACITY, LegacyAdvertisingReport, LegacyAdvertisingReportKind},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LegacyAdvertisingDuplicateIdentity {
    kind: LegacyAdvertisingReportKind,
    advertiser: LeDeviceAddress,
    directed_target: Option<LeDeviceAddress>,
    data: [u8; LEGACY_SCAN_DATA_CAPACITY],
    data_len: u8,
}

fn duplicate_identity(report: LegacyAdvertisingReport) -> LegacyAdvertisingDuplicateIdentity {
    let mut data = [0; LEGACY_SCAN_DATA_CAPACITY];
    for (slot, byte) in data.iter_mut().zip(report.data()) {
        *slot = *byte;
    }
    LegacyAdvertisingDuplicateIdentity {
        kind: report.kind(),
        advertiser: report.advertiser(),
        directed_target: report.directed_target(),
        data,
        data_len: report.data().len() as u8,
    }
}

/// Fixed exact-match duplicate cache. RSSI and receive channel intentionally do
/// not affect identity; changed advertising data does.
pub struct LegacyAdvertisingDuplicateFilter<const CAPACITY: usize> {
    entries: [Option<LegacyAdvertisingDuplicateIdentity>; CAPACITY],
}

impl<const CAPACITY: usize> LegacyAdvertisingDuplicateFilter<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            entries: [None; CAPACITY],
        }
    }

    /// Return true exactly once for a new identity while retaining it.
    pub fn accept(&mut self, report: LegacyAdvertisingReport) -> bool {
        let identity = duplicate_identity(report);
        if self
            .entries
            .iter()
            .flatten()
            .any(|entry| *entry == identity)
        {
            return false;
        }
        // Newest first, oldest last: the same FIFO eviction without a cursor
        // whose range or wrap must be checked. A zero-capacity cache stores nothing.
        let mut next = Some(identity);
        for slot in &mut self.entries {
            core::mem::swap(slot, &mut next);
        }
        true
    }

    pub fn clear(&mut self) {
        self.entries.fill(None);
    }
}

impl<const CAPACITY: usize> Default for LegacyAdvertisingDuplicateFilter<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
mod tests;
