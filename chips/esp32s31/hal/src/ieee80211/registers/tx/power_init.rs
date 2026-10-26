//! Generated-PAC ownership for the complete cold MAC TX-power publication.

use crate::ieee80211::registers::{MacPlacement, WifiRadioRegisters, bits, byte, element};

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
        _ => panic!("the selector constructor admits no other encoding"),
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
                let register = init.read_tb_ru_power(element::<3>(word));
                match lane {
                    0 => register.power_0(),
                    1 => register.power_1(),
                    2 => register.power_2(),
                    3 => register.power_3(),
                    4 => register.power_4(),
                    _ => unreachable!("a packed RU power lane is below five"),
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
        let value = bits::<6>(u32::from(power.value()));
        match partial_ru_power_slot(selector) {
            PartialRuPowerSlot::Packed { word, lane } => {
                init.modify_tb_ru_power(element::<3>(word), |w| {
                    match lane {
                        0 => w.set_power_0(value),
                        1 => w.set_power_1(value),
                        2 => w.set_power_2(value),
                        3 => w.set_power_3(value),
                        4 => w.set_power_4(value),
                        _ => unreachable!("a packed RU power lane is below five"),
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
        let index = |rate: u8| bits::<6>(u32::from(table.primary_index(rate)));

        // Complete hal_init_tb_power: rates 16..=25, one six-bit field per RMW.
        let tb0 = element::<3>(0);
        init.modify_tb_power(tb0, |w| {
            w.set_power_0(index(16));
        });
        init.modify_tb_power(tb0, |w| {
            w.set_power_1(index(17));
        });
        init.modify_tb_power(tb0, |w| {
            w.set_power_2(index(18));
        });
        init.modify_tb_power(tb0, |w| {
            w.set_power_3(index(19));
        });
        let tb1 = element::<3>(1);
        init.modify_tb_power(tb1, |w| {
            w.set_power_0(index(20));
        });
        init.modify_tb_power(tb1, |w| {
            w.set_power_1(index(21));
        });
        init.modify_tb_power(tb1, |w| {
            w.set_power_2(index(22));
        });
        init.modify_tb_power(tb1, |w| {
            w.set_power_3(index(23));
        });
        let tb2 = element::<3>(2);
        init.modify_tb_power(tb2, |w| {
            w.set_power_0(index(24));
        });
        init.modify_tb_power(tb2, |w| {
            w.set_power_1(index(25));
        });

        // Complete hal_init_imrsp_power. Each tuple is
        // (format, encoded rate, table lookup rate). The blob replaces these
        // three fields through three separate RMWs for every command word.
        const RESPONSE: [(u8, u8, u8); 10] = [
            (
                oer_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format0.bits() as u8,
                oer_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate00.bits() as u8,
                0,
            ),
            (
                oer_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format0.bits() as u8,
                oer_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate01.bits() as u8,
                0,
            ),
            (
                oer_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format0.bits() as u8,
                oer_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate05.bits() as u8,
                5,
            ),
            (
                oer_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format0.bits() as u8,
                oer_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate0b.bits() as u8,
                11,
            ),
            (
                oer_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format0.bits() as u8,
                oer_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate0a.bits() as u8,
                10,
            ),
            (
                oer_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format0.bits() as u8,
                oer_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate09.bits() as u8,
                9,
            ),
            (
                oer_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format2.bits() as u8,
                oer_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate10.bits() as u8,
                16,
            ),
            (
                oer_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format2.bits() as u8,
                oer_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate11.bits() as u8,
                17,
            ),
            (
                oer_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format2.bits() as u8,
                oer_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate12.bits() as u8,
                18,
            ),
            (
                oer_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format2.bits() as u8,
                oer_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate12.bits() as u8,
                18,
            ),
        ];
        for (word, (format, encoded_rate, lookup_rate)) in RESPONSE.into_iter().enumerate() {
            let response = element::<10>(word);
            init.modify_immediate_response(response, |w| {
                w.set_sig_mode(bits(u32::from(format)));
            });
            init.modify_immediate_response(response, |w| {
                w.set_rate_index(bits(u32::from(encoded_rate)));
            });
            init.modify_immediate_response(response, |w| {
                w.set_power_index(index(lookup_rate));
            });
        }

        // Complete hal_init_tb_ru_power and the complete setter jump tables.
        // The complete leaf zero-extends the signed table byte before each
        // unsigned clamp. Do not truncate to the six-bit destination field
        // until after subtraction: negative i8 values therefore retain the
        // blob's modulo-256 then modulo-64 behavior.
        let base = table.primary_byte(16);
        let delta_10 = bits::<6>(u32::from(relative_index(base, 10)));
        let delta_7 = bits::<6>(u32::from(relative_index(base, 7)));
        let delta_4 = bits::<6>(u32::from(relative_index(base, 4)));

        // Selectors 0..=8.
        let ru0 = element::<3>(0);
        init.modify_tb_ru_power(ru0, |w| {
            w.set_power_0(delta_10);
        });
        init.modify_tb_ru_power(ru0, |w| {
            w.set_power_1(delta_10);
        });
        init.modify_tb_ru_power(ru0, |w| {
            w.set_power_2(delta_10);
        });
        init.modify_tb_ru_power(ru0, |w| {
            w.set_power_3(delta_10);
        });
        init.modify_tb_ru_power(ru0, |w| {
            w.set_power_4(delta_10);
        });
        let ru1 = element::<3>(1);
        init.modify_tb_ru_power(ru1, |w| {
            w.set_power_0(delta_10);
        });
        init.modify_tb_ru_power(ru1, |w| {
            w.set_power_1(delta_10);
        });
        init.modify_tb_ru_power(ru1, |w| {
            w.set_power_2(delta_10);
        });
        init.modify_tb_ru_power(ru1, |w| {
            w.set_power_3(delta_10);
        });

        // Selectors 37..=40.
        init.modify_tb_ru_power(ru1, |w| {
            w.set_power_4(delta_7);
        });
        let ru2 = element::<3>(2);
        init.modify_tb_ru_power(ru2, |w| {
            w.set_power_0(delta_7);
        });
        init.modify_tb_ru_power(ru2, |w| {
            w.set_power_1(delta_7);
        });
        init.modify_tb_ru_power(ru2, |w| {
            w.set_power_2(delta_7);
        });

        // Selectors 53, 54 and 61.
        init.modify_tb_ru_power(ru2, |w| {
            w.set_power_3(delta_4);
        });
        init.modify_tb_ru_power(ru2, |w| {
            w.set_power_4(delta_4);
        });
        init.modify_tb_ru_power_tail(|w| {
            w.set_power_index(bits(u32::from(base & 0x3f)));
        });
    }
}

#[cfg(test)]
mod tests;
