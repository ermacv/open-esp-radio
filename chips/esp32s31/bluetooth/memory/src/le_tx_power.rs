//! Requested LE transmit power as an index into the provider's level table.

#![forbid(unsafe_code)]

use oer_esp32s31_hal::phy::baseband::TX_POWER_LEVELS_DBM;

/// One LE transmit-power request the controller can encode.
///
/// SOURCE: pinned `libble_app.a` member `ble_70.o`
/// `r_sym_ble_5YIHPyqYswSaQs7KMvEA`. With the BLE PHY environment's level
/// table installed, as `r_sym_ble_ragNLqDkbVnj7GUutTgs` does from the BTBB
/// provider, it searches the table from its highest level down and returns
/// the index of the first level not above the request. A request below the
/// lowest level matches none and asserts; this type refuses it instead. The
/// index never crosses the memory-codec boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeTxPower(u8);

impl LeTxPower {
    /// The lowest level, in dBm: every request at or above it selects a
    /// level.
    pub const LOWEST_DBM: i8 = TX_POWER_LEVELS_DBM[0];

    /// The highest provider level not above `dbm`, or `None` below the
    /// lowest level.
    pub const fn from_dbm(dbm: i8) -> Option<Self> {
        let mut index = TX_POWER_LEVELS_DBM.len();
        while index > 0 {
            index -= 1;
            if TX_POWER_LEVELS_DBM[index] <= dbm {
                return Some(Self(index as u8));
            }
        }
        None
    }

    /// The provider level this request selects, in dBm.
    pub const fn level_dbm(self) -> i8 {
        TX_POWER_LEVELS_DBM[self.0 as usize]
    }

    pub(super) const fn index(self) -> u8 {
        self.0
    }
}

#[cfg(test)]
mod tests;
