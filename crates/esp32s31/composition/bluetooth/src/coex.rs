//! Bluetooth LE status in the shared coexistence schedule.
//!
//! The Controller reports which roles it has active; the runner publishes
//! each change to the radio system as Bluetooth LE schedule status bits, and
//! the stop withdraws them all.

use oer_bluetooth_radio::RadioActivity;

/// Bluetooth LE schedule status for active advertising.
///
/// SOURCE: pinned `libble_app.a` `coexHook.c.o_1.o`
/// `r_sym_coexHook_QgUvVmt2sVpZ9D6zbl3P`, the default status hook installed
/// by `r_sym_coexHook_RqzHsepjtcxRQpoBqpK6`. It maps the eleven role indices
/// through `sym_coexHook_Gup5twTY9KpBwx4sNJzY` to status groups and the
/// groups through `sym_coexHook_jdG4gVxboQLZAqcqm7Ys` = `01 02 04 ff`: legacy
/// and extended advertising (indices 0 and 2) and periodic advertising (4)
/// share bit 0, scanning and initiating bit 1, connections (7) bit 2. The
/// first user of a group sets its bit through
/// `wr_btdm_coex_schm_status_bit_set(1, bit)` and the last clears it; index
/// 10 clears every bit on task disable.
pub(crate) const BLE_COEX_STATUS_ADVERTISING: u16 = 0x01;
/// Bluetooth LE schedule status for an active scanner or initiator.
pub(crate) const BLE_COEX_STATUS_SCANNING: u16 = 0x02;
/// Bluetooth LE schedule status for an active connection.
pub(crate) const BLE_COEX_STATUS_CONNECTED: u16 = 0x04;

/// The schedule status bits of `activity`.
pub(crate) const fn ble_coex_status(activity: RadioActivity) -> u16 {
    let mut bits = 0;
    if activity.advertising {
        bits |= BLE_COEX_STATUS_ADVERTISING;
    }
    if activity.scanning {
        bits |= BLE_COEX_STATUS_SCANNING;
    }
    if activity.connected {
        bits |= BLE_COEX_STATUS_CONNECTED;
    }
    bits
}

/// The status bits to withdraw and to publish when the active roles move
/// from one summary to the next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BleCoexStatusChange {
    /// Bits no role holds any more.
    pub(crate) clear: u16,
    /// Bits a role now holds for the first time.
    pub(crate) set: u16,
}

impl BleCoexStatusChange {
    /// The change from `published` to `activity`.
    pub(crate) const fn between(published: RadioActivity, activity: RadioActivity) -> Self {
        let (old, new) = (ble_coex_status(published), ble_coex_status(activity));
        Self {
            clear: old & !new,
            set: new & !old,
        }
    }
}

#[cfg(test)]
mod tests;
