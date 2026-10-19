//! Bluetooth LE status in the shared coexistence schedule, recovered from the
//! vendor status hook.
//!
//! A Controller publishes which roles it has active as Bluetooth LE schedule
//! status bits, and its stop withdraws them all. No composition in this
//! repository publishes them.

/// The long-lived roles a Controller has active.
///
/// A role is active from the moment the Host enables it until the Controller
/// has stopped it: advertising and scanning while enabled or still stopping,
/// a connection from its creation until it closes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RadioActivity {
    /// An advertising set is active.
    pub advertising: bool,
    /// The scanner is active.
    pub scanning: bool,
    /// A connection is active.
    pub connected: bool,
}

impl RadioActivity {
    /// No role active.
    pub const IDLE: Self = Self {
        advertising: false,
        scanning: false,
        connected: false,
    };
}

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
pub const BLE_COEX_STATUS_ADVERTISING: u16 = 0x01;
/// Bluetooth LE schedule status for an active scanner or initiator.
pub const BLE_COEX_STATUS_SCANNING: u16 = 0x02;
/// Bluetooth LE schedule status for an active connection.
pub const BLE_COEX_STATUS_CONNECTED: u16 = 0x04;

/// The schedule status bits of `activity`.
pub const fn ble_coex_status(activity: RadioActivity) -> u16 {
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
pub struct BleCoexStatusChange {
    /// Bits no role holds any more.
    pub clear: u16,
    /// Bits a role now holds for the first time.
    pub set: u16,
}

impl BleCoexStatusChange {
    /// The change from `published` to `activity`.
    pub const fn between(published: RadioActivity, activity: RadioActivity) -> Self {
        let (old, new) = (ble_coex_status(published), ble_coex_status(activity));
        Self {
            clear: old & !new,
            set: new & !old,
        }
    }
}

#[cfg(test)]
mod tests;
