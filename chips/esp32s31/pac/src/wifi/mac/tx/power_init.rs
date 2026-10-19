//! Generated-PAC ownership for the complete cold MAC TX-power publication.

#![forbid(unsafe_code)]

use crate::WifiRadioRegisters;

/// Number of two-byte rate entries produced by complete `hal_init_tx_pwr`.
pub const MAC_TX_POWER_RATE_COUNT: usize = 43;

/// One six-bit PHY gain-table index stored by the MAC.
///
/// This is not dBm. The private field prevents safe upper layers from
/// truncating an arbitrary byte into a six-bit MMIO field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxPowerIndex(u8);

impl MacTxPowerIndex {
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 0x3f {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// A Trigger RU allocation for which the S31 exposes runtime TX-power state.
///
/// The encoding is the seven-bit RU allocation carried in Trigger User Info.
/// It is deliberately a validated newtype rather than an enum with invented
/// register-oriented names: callers retain the over-air value while invalid
/// gaps cannot reach MMIO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacPartialRuPowerSelector(u8);

impl MacPartialRuPowerSelector {
    /// Admit exactly the selector set accepted by the complete vendor HAL.
    ///
    /// SOURCE: `libnet80211.a[ieee80211_api.o]` complete
    /// `esp_wifi_internal_{get,set}_partial_ru_max_tx_pwr` and
    /// `[ieee80211_ioctl.o]` process leaves pass this byte unchanged to
    /// complete `libpp.a[hal_mac_ctl.o]::
    /// hal_mac_{get,set}_tb_max_pwr`. Both relocation-complete jump tables
    /// accept 0..=8, 37..=40, 53, 54 and 61 and reject every other value.
    pub const fn from_trigger_encoding(value: u8) -> Option<Self> {
        match value {
            0..=8 | 37..=40 | 53 | 54 | 61 => Some(Self(value)),
            _ => None,
        }
    }

    pub const fn trigger_encoding(self) -> u8 {
        self.0
    }
}

/// Calibrated PHY gain-table indices for one MAC rate.
///
/// These values are signed indices after the ROM divides its quarter-unit
/// target-power values by four. They are not dBm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxPowerPair {
    pub primary: i8,
    pub alternate: i8,
}

impl MacTxPowerPair {
    pub const ZERO: Self = Self {
        primary: 0,
        alternate: 0,
    };
}

/// Rust-owned replacement for vendor `s_phy_get_max_pwr[43][2]`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxPowerTable {
    entries: [MacTxPowerPair; MAC_TX_POWER_RATE_COUNT],
}

impl MacTxPowerTable {
    pub const fn new(entries: [MacTxPowerPair; MAC_TX_POWER_RATE_COUNT]) -> Self {
        Self { entries }
    }

    pub const fn pair(&self, rate: u8) -> Option<MacTxPowerPair> {
        let index = rate as usize;
        if index < MAC_TX_POWER_RATE_COUNT {
            Some(self.entries[index])
        } else {
            None
        }
    }

    const fn primary_byte(&self, rate: u8) -> u8 {
        // Every rate used by the three cold leaves is in 0..=25. Preserve the
        // complete hal_get_tx_pwr remap as part of the owned table API because
        // the same table later formats HE rates above 25.
        let table_rate = if rate > 25 {
            rate.wrapping_sub(10)
        } else {
            rate
        };
        self.entries[table_rate as usize].primary as u8
    }

    const fn primary_index(&self, rate: u8) -> u8 {
        self.primary_byte(rate) & 0x3f
    }
}

fn relative_index(base: u8, floor: u8) -> u8 {
    base.max(floor).wrapping_sub(floor) & 0x3f
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PartialRuPowerSlot {
    Packed { word: usize, lane: u8 },
    Tail,
}

const fn partial_ru_power_slot(selector: MacPartialRuPowerSelector) -> PartialRuPowerSlot {
    match selector.trigger_encoding() {
        raw @ 0..=4 => PartialRuPowerSlot::Packed { word: 0, lane: raw },
        raw @ 5..=8 => PartialRuPowerSlot::Packed {
            word: 1,
            lane: raw - 5,
        },
        37 => PartialRuPowerSlot::Packed { word: 1, lane: 4 },
        raw @ 38..=40 => PartialRuPowerSlot::Packed {
            word: 2,
            lane: raw - 38,
        },
        53 => PartialRuPowerSlot::Packed { word: 2, lane: 3 },
        54 => PartialRuPowerSlot::Packed { word: 2, lane: 4 },
        61 => PartialRuPowerSlot::Tail,
        // The selector constructor makes this arm unreachable.
        _ => unreachable!(),
    }
}

impl WifiRadioRegisters {
    /// Read the runtime maximum TX-power index for one partial RU.
    ///
    /// SOURCE: complete `hal_mac_get_tb_max_pwr` including both relocation
    /// jump tables. The returned six-bit value is a PHY gain-table index, not
    /// a radiated dBm value.
    pub fn partial_ru_max_tx_power(&self, selector: MacPartialRuPowerSelector) -> MacTxPowerIndex {
        let init = &self.peripherals.wifi_mac.wifi_mac_tx_power_init;
        let value = match partial_ru_power_slot(selector) {
            PartialRuPowerSlot::Packed { word, lane } => {
                let register = init.tb_ru_power(word).read();
                match lane {
                    0 => register.power_0().bits(),
                    1 => register.power_1().bits(),
                    2 => register.power_2().bits(),
                    3 => register.power_3().bits(),
                    4 => register.power_4().bits(),
                    _ => unreachable!(),
                }
            }
            PartialRuPowerSlot::Tail => init.tb_ru_power_tail().read().power_index().bits(),
        };
        // Every generated field reader above is exactly six bits wide.
        MacTxPowerIndex(value)
    }

    /// Replace the runtime maximum TX-power index for one partial RU.
    ///
    /// SOURCE: complete `hal_mac_set_tb_max_pwr` including both relocation
    /// jump tables. It performs one fresh-read RMW of the selected six-bit
    /// field and preserves all adjacent RU values.
    pub fn set_partial_ru_max_tx_power(
        &mut self,
        selector: MacPartialRuPowerSelector,
        power: MacTxPowerIndex,
    ) {
        let init = &self.peripherals.wifi_mac.wifi_mac_tx_power_init;
        match partial_ru_power_slot(selector) {
            PartialRuPowerSlot::Packed { word, lane } => {
                let register = init.tb_ru_power(word);
                match lane {
                    0 => register.modify(|_, w| w.power_0().set(power.value())),
                    1 => register.modify(|_, w| w.power_1().set(power.value())),
                    2 => register.modify(|_, w| w.power_2().set(power.value())),
                    3 => register.modify(|_, w| w.power_3().set(power.value())),
                    4 => register.modify(|_, w| w.power_4().set(power.value())),
                    _ => unreachable!(),
                };
            }
            PartialRuPowerSlot::Tail => {
                init.tb_ru_power_tail()
                    .modify(|_, w| w.power_index().set(power.value()));
            }
        }
    }

    /// Publish the complete TB, immediate-response and TB-RU power tables.
    ///
    /// SOURCE: complete pinned `libpp.a` functions recorded as
    /// `BLOB_LIBPP_HAL_TX_POWER_INIT`. This method preserves all 56
    /// fresh-read RMW edges and their original child-call order.
    pub fn initialize_mac_tx_power(&mut self, table: &MacTxPowerTable) {
        let init = &self.peripherals.wifi_mac.wifi_mac_tx_power_init;

        // Complete hal_init_tb_power: rates 16..=25, one six-bit field per RMW.
        let tb0 = init.tb_power(0);
        tb0.modify(|_, w| w.power_0().set(table.primary_index(16)));
        tb0.modify(|_, w| w.power_1().set(table.primary_index(17)));
        tb0.modify(|_, w| w.power_2().set(table.primary_index(18)));
        tb0.modify(|_, w| w.power_3().set(table.primary_index(19)));
        let tb1 = init.tb_power(1);
        tb1.modify(|_, w| w.power_0().set(table.primary_index(20)));
        tb1.modify(|_, w| w.power_1().set(table.primary_index(21)));
        tb1.modify(|_, w| w.power_2().set(table.primary_index(22)));
        tb1.modify(|_, w| w.power_3().set(table.primary_index(23)));
        let tb2 = init.tb_power(2);
        tb2.modify(|_, w| w.power_0().set(table.primary_index(24)));
        tb2.modify(|_, w| w.power_1().set(table.primary_index(25)));

        // Complete hal_init_imrsp_power. Each tuple is
        // (format, encoded rate, table lookup rate). The blob replaces these
        // three fields through three separate RMWs for every command word.
        const RESPONSE: [(u8, u8, u8); 10] = [
            (0, 0, 0),
            (0, 1, 0),
            (0, 5, 5),
            (0, 11, 11),
            (0, 10, 10),
            (0, 9, 9),
            (2, 16, 16),
            (2, 17, 17),
            (2, 18, 18),
            (2, 18, 18),
        ];
        for (word, (format, encoded_rate, lookup_rate)) in RESPONSE.into_iter().enumerate() {
            let response = init.immediate_response(word);
            response.modify(|_, w| w.bits_22_23_opaque().set(format));
            response.modify(|_, w| w.rate_index().set(encoded_rate));
            response.modify(|_, w| w.power_index().set(table.primary_index(lookup_rate)));
        }

        // Complete hal_init_tb_ru_power and the complete setter jump tables.
        // The complete leaf zero-extends the signed table byte before each
        // unsigned clamp. Do not truncate to the six-bit destination field
        // until after subtraction: negative i8 values therefore retain the
        // blob's modulo-256 then modulo-64 behavior.
        let base = table.primary_byte(16);
        let delta_10 = relative_index(base, 10);
        let delta_7 = relative_index(base, 7);
        let delta_4 = relative_index(base, 4);

        // Selectors 0..=8.
        let ru0 = init.tb_ru_power(0);
        ru0.modify(|_, w| w.power_0().set(delta_10));
        ru0.modify(|_, w| w.power_1().set(delta_10));
        ru0.modify(|_, w| w.power_2().set(delta_10));
        ru0.modify(|_, w| w.power_3().set(delta_10));
        ru0.modify(|_, w| w.power_4().set(delta_10));
        let ru1 = init.tb_ru_power(1);
        ru1.modify(|_, w| w.power_0().set(delta_10));
        ru1.modify(|_, w| w.power_1().set(delta_10));
        ru1.modify(|_, w| w.power_2().set(delta_10));
        ru1.modify(|_, w| w.power_3().set(delta_10));

        // Selectors 37..=40.
        ru1.modify(|_, w| w.power_4().set(delta_7));
        let ru2 = init.tb_ru_power(2);
        ru2.modify(|_, w| w.power_0().set(delta_7));
        ru2.modify(|_, w| w.power_1().set(delta_7));
        ru2.modify(|_, w| w.power_2().set(delta_7));

        // Selectors 53, 54 and 61.
        ru2.modify(|_, w| w.power_3().set(delta_4));
        ru2.modify(|_, w| w.power_4().set(delta_4));
        init.tb_ru_power_tail()
            .modify(|_, w| w.power_index().set(base & 0x3f));
    }
}

#[cfg(test)]
mod tests;
