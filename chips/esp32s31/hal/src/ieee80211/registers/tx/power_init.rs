//! The complete cold MAC TX-power publication and the partial-RU power
//! limits.

use oer_mmio::{Bits, Index};

use crate::ieee80211::registers::{MacPlacement, WifiRadioRegisters, byte, every_index};

/// The ten immediate-response command words.
const RESPONSE_WORDS: [Index<10>; 10] = every_index();

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
        match self.pair(table_rate) {
            Some(pair) => pair.primary.to_le_bytes()[0],
            // Every remapped rate lies below the table's 43 entries.
            None => 0,
        }
    }

    const fn primary_index(&self, rate: u8) -> Bits<6> {
        Bits::truncate(self.primary_byte(rate) as u32)
    }
}

fn relative_index(base: u8, floor: u8) -> Bits<6> {
    Bits::truncate(u32::from(base.max(floor).wrapping_sub(floor)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PartialRuPowerSlot {
    Packed { word: Index<3>, lane: u8 },
    Tail,
}

const fn partial_ru_power_slot(selector: MacPartialRuPowerSelector) -> PartialRuPowerSlot {
    match selector.trigger_encoding() {
        raw @ 0..=4 => PartialRuPowerSlot::Packed {
            word: Index::constant::<0>(),
            lane: raw,
        },
        raw @ 5..=8 => PartialRuPowerSlot::Packed {
            word: Index::constant::<1>(),
            lane: raw - 5,
        },
        37 => PartialRuPowerSlot::Packed {
            word: Index::constant::<1>(),
            lane: 4,
        },
        raw @ 38..=40 => PartialRuPowerSlot::Packed {
            word: Index::constant::<2>(),
            lane: raw - 38,
        },
        53 => PartialRuPowerSlot::Packed {
            word: Index::constant::<2>(),
            lane: 3,
        },
        54 => PartialRuPowerSlot::Packed {
            word: Index::constant::<2>(),
            lane: 4,
        },
        61 => PartialRuPowerSlot::Tail,
        // The selector constructor admits only 61 besides the arms above.
        _ => PartialRuPowerSlot::Tail,
    }
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Read the runtime maximum TX-power index for one partial RU.
    ///
    /// SOURCE: complete `hal_mac_get_tb_max_pwr` including both relocation
    /// jump tables. The returned six-bit value is a PHY gain-table index, not
    /// a radiated dBm value.
    pub fn partial_ru_max_tx_power(&self, selector: MacPartialRuPowerSelector) -> MacTxPowerIndex {
        let init = &self.mac.wifi_mac_tx_power_init;
        let value = match partial_ru_power_slot(selector) {
            PartialRuPowerSlot::Packed { word, lane } => {
                let register = init.read_tb_ru_power(word);
                match lane {
                    0 => register.power_0(),
                    1 => register.power_1(),
                    2 => register.power_2(),
                    3 => register.power_3(),
                    _ => register.power_4(),
                }
            }
            PartialRuPowerSlot::Tail => init.read_tb_ru_power_tail().power_index(),
        };
        // Every field read above is exactly six bits wide.
        MacTxPowerIndex(byte(value))
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
        let init = &mut self.mac.wifi_mac_tx_power_init;
        let value = Bits::<6>::truncate(u32::from(power.value()));
        match partial_ru_power_slot(selector) {
            PartialRuPowerSlot::Packed { word, lane } => {
                init.modify_tb_ru_power(word, |w| {
                    match lane {
                        0 => w.set_power_0(value),
                        1 => w.set_power_1(value),
                        2 => w.set_power_2(value),
                        3 => w.set_power_3(value),
                        _ => w.set_power_4(value),
                    };
                });
            }
            PartialRuPowerSlot::Tail => {
                init.modify_tb_ru_power_tail(|w| {
                    w.set_power_index(value);
                });
            }
        }
    }

    /// Publish the complete TB, immediate-response and TB-RU power tables.
    ///
    /// SOURCE: complete pinned `libpp.a` functions recorded as
    /// `BLOB_LIBPP_HAL_TX_POWER_INIT`. This method preserves all 56
    /// fresh-read RMW edges and their original child-call order.
    pub fn initialize_mac_tx_power(&mut self, table: &MacTxPowerTable) {
        let init = &mut self.mac.wifi_mac_tx_power_init;
        let [tb0, tb1, tb2] = [
            Index::constant::<0>(),
            Index::constant::<1>(),
            Index::constant::<2>(),
        ];

        // Complete hal_init_tb_power: rates 16..=25, one six-bit field per RMW.
        init.modify_tb_power(tb0, |w| {
            w.set_power_0(table.primary_index(16));
        });
        init.modify_tb_power(tb0, |w| {
            w.set_power_1(table.primary_index(17));
        });
        init.modify_tb_power(tb0, |w| {
            w.set_power_2(table.primary_index(18));
        });
        init.modify_tb_power(tb0, |w| {
            w.set_power_3(table.primary_index(19));
        });
        init.modify_tb_power(tb1, |w| {
            w.set_power_0(table.primary_index(20));
        });
        init.modify_tb_power(tb1, |w| {
            w.set_power_1(table.primary_index(21));
        });
        init.modify_tb_power(tb1, |w| {
            w.set_power_2(table.primary_index(22));
        });
        init.modify_tb_power(tb1, |w| {
            w.set_power_3(table.primary_index(23));
        });
        init.modify_tb_power(tb2, |w| {
            w.set_power_0(table.primary_index(24));
        });
        init.modify_tb_power(tb2, |w| {
            w.set_power_1(table.primary_index(25));
        });

        // Complete hal_init_imrsp_power. Each tuple is
        // (format, encoded rate, table lookup rate). The blob replaces these
        // three fields through three separate RMWs for every command word.
        const RESPONSE: [(Bits<2>, Bits<6>, u8); 10] = [
            (Bits::constant::<0>(), Bits::constant::<0>(), 0),
            (Bits::constant::<0>(), Bits::constant::<1>(), 0),
            (Bits::constant::<0>(), Bits::constant::<5>(), 5),
            (Bits::constant::<0>(), Bits::constant::<11>(), 11),
            (Bits::constant::<0>(), Bits::constant::<10>(), 10),
            (Bits::constant::<0>(), Bits::constant::<9>(), 9),
            (Bits::constant::<2>(), Bits::constant::<16>(), 16),
            (Bits::constant::<2>(), Bits::constant::<17>(), 17),
            (Bits::constant::<2>(), Bits::constant::<18>(), 18),
            (Bits::constant::<2>(), Bits::constant::<18>(), 18),
        ];
        for (word, (format, encoded_rate, lookup_rate)) in RESPONSE_WORDS.into_iter().zip(RESPONSE)
        {
            init.modify_immediate_response(word, |w| {
                w.set_bits_22_23_opaque(format);
            });
            init.modify_immediate_response(word, |w| {
                w.set_rate_index(encoded_rate);
            });
            init.modify_immediate_response(word, |w| {
                w.set_power_index(table.primary_index(lookup_rate));
            });
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
        init.modify_tb_ru_power(tb0, |w| {
            w.set_power_0(delta_10);
        });
        init.modify_tb_ru_power(tb0, |w| {
            w.set_power_1(delta_10);
        });
        init.modify_tb_ru_power(tb0, |w| {
            w.set_power_2(delta_10);
        });
        init.modify_tb_ru_power(tb0, |w| {
            w.set_power_3(delta_10);
        });
        init.modify_tb_ru_power(tb0, |w| {
            w.set_power_4(delta_10);
        });
        init.modify_tb_ru_power(tb1, |w| {
            w.set_power_0(delta_10);
        });
        init.modify_tb_ru_power(tb1, |w| {
            w.set_power_1(delta_10);
        });
        init.modify_tb_ru_power(tb1, |w| {
            w.set_power_2(delta_10);
        });
        init.modify_tb_ru_power(tb1, |w| {
            w.set_power_3(delta_10);
        });

        // Selectors 37..=40.
        init.modify_tb_ru_power(tb1, |w| {
            w.set_power_4(delta_7);
        });
        init.modify_tb_ru_power(tb2, |w| {
            w.set_power_0(delta_7);
        });
        init.modify_tb_ru_power(tb2, |w| {
            w.set_power_1(delta_7);
        });
        init.modify_tb_ru_power(tb2, |w| {
            w.set_power_2(delta_7);
        });

        // Selectors 53, 54 and 61.
        init.modify_tb_ru_power(tb2, |w| {
            w.set_power_3(delta_4);
        });
        init.modify_tb_ru_power(tb2, |w| {
            w.set_power_4(delta_4);
        });
        init.modify_tb_ru_power_tail(|w| {
            w.set_power_index(Bits::truncate(u32::from(base)));
        });
    }
}

#[cfg(test)]
mod tests;
