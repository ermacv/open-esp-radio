//! Generated-PAC ownership for the complete cold MAC TX-power publication.

use crate::radio::ieee80211::registers::{MacPlacement, WifiRadioRegisters, element};

/// Number of two-byte rate entries produced by complete `hal_init_tx_pwr`.
pub const MAC_TX_POWER_RATE_COUNT: usize = 43;

/// One six-bit PHY gain-table index stored by the MAC.
///
/// This is not dBm. The private field prevents safe upper layers from
/// truncating an arbitrary byte into a six-bit MMIO field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxPowerIndex(
    oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::tb_ru_power_tail::POWER_INDEX_WIDTH },
    >,
);

impl MacTxPowerIndex {
    pub const fn new(value: u8) -> Option<Self> {
        match oer_portable_mmio::Bits::new(value as u32) {
            Ok(bits) => Some(Self(bits)),
            Err(_) => None,
        }
    }

    pub const fn value(self) -> u8 {
        self.0.get() as u8
    }

    /// The index as the six-bit RU power fields hold it.
    const fn field(
        self,
    ) -> oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::tb_ru_power_tail::POWER_INDEX_WIDTH },
    > {
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
}

/// The RU delta of `base` above `floor`, clamped at zero, as the vendor's
/// byte; the field's vendor cut keeps its low six bits.
fn relative_index(base: u8, floor: u8) -> u8 {
    base.max(floor).wrapping_sub(floor)
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
        let value = power.field();
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
        let index = |rate: u8| table.primary_byte(rate);

        // Complete hal_init_tb_power: rates 16..=25, one six-bit field per RMW.
        let tb0 = element::<3>(0);
        init.modify_tb_power(tb0, |w| {
            w.set_tb_power_0_low_bits(index(16));
        });
        init.modify_tb_power(tb0, |w| {
            w.set_tb_power_1_low_bits(index(17));
        });
        init.modify_tb_power(tb0, |w| {
            w.set_tb_power_2_low_bits(index(18));
        });
        init.modify_tb_power(tb0, |w| {
            w.set_tb_power_3_low_bits(index(19));
        });
        let tb1 = element::<3>(1);
        init.modify_tb_power(tb1, |w| {
            w.set_tb_power_0_low_bits(index(20));
        });
        init.modify_tb_power(tb1, |w| {
            w.set_tb_power_1_low_bits(index(21));
        });
        init.modify_tb_power(tb1, |w| {
            w.set_tb_power_2_low_bits(index(22));
        });
        init.modify_tb_power(tb1, |w| {
            w.set_tb_power_3_low_bits(index(23));
        });
        let tb2 = element::<3>(2);
        init.modify_tb_power(tb2, |w| {
            w.set_tb_power_0_low_bits(index(24));
        });
        init.modify_tb_power(tb2, |w| {
            w.set_tb_power_1_low_bits(index(25));
        });

        // Complete hal_init_imrsp_power. Each tuple is
        // (format, encoded rate, table lookup rate). The blob replaces these
        // three fields through three separate RMWs for every command word.
        const RESPONSE: [(
            oer_chips_esp32s31_pac::wifi_mac::immediate_response::SigMode,
            oer_chips_esp32s31_pac::wifi_mac::immediate_response::RateIndex,
            u8,
        ); 10] = [
            (
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format0,
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate00,
                0,
            ),
            (
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format0,
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate01,
                0,
            ),
            (
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format0,
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate05,
                5,
            ),
            (
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format0,
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate0b,
                11,
            ),
            (
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format0,
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate0a,
                10,
            ),
            (
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format0,
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate09,
                9,
            ),
            (
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format2,
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate10,
                16,
            ),
            (
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format2,
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate11,
                17,
            ),
            (
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format2,
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate12,
                18,
            ),
            (
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::SigMode::Format2,
                oer_chips_esp32s31_pac::wifi_mac::immediate_response::RateIndex::Rate12,
                18,
            ),
        ];
        for (word, (format, encoded_rate, lookup_rate)) in RESPONSE.into_iter().enumerate() {
            let response = element::<10>(word);
            init.modify_immediate_response(response, |w| {
                w.set_sig_mode(format.into());
            });
            init.modify_immediate_response(response, |w| {
                w.set_rate_index(encoded_rate.into());
            });
            init.modify_immediate_response(response, |w| {
                w.set_response_power_index_low_bits(index(lookup_rate));
            });
        }

        // Complete hal_init_tb_ru_power and the complete setter jump tables.
        // The complete leaf zero-extends the signed table byte before each
        // unsigned clamp. The six-bit cut is the fields' vendor cut, after
        // the subtraction: negative i8 values therefore retain the blob's
        // modulo-256 then modulo-64 behavior.
        let base = table.primary_byte(16);
        let delta_10 = relative_index(base, 10);
        let delta_7 = relative_index(base, 7);
        let delta_4 = relative_index(base, 4);

        // Selectors 0..=8.
        let ru0 = element::<3>(0);
        init.modify_tb_ru_power(ru0, |w| {
            w.set_tb_ru_power_0_low_bits(delta_10);
        });
        init.modify_tb_ru_power(ru0, |w| {
            w.set_tb_ru_power_1_low_bits(delta_10);
        });
        init.modify_tb_ru_power(ru0, |w| {
            w.set_tb_ru_power_2_low_bits(delta_10);
        });
        init.modify_tb_ru_power(ru0, |w| {
            w.set_tb_ru_power_3_low_bits(delta_10);
        });
        init.modify_tb_ru_power(ru0, |w| {
            w.set_tb_ru_power_4_low_bits(delta_10);
        });
        let ru1 = element::<3>(1);
        init.modify_tb_ru_power(ru1, |w| {
            w.set_tb_ru_power_0_low_bits(delta_10);
        });
        init.modify_tb_ru_power(ru1, |w| {
            w.set_tb_ru_power_1_low_bits(delta_10);
        });
        init.modify_tb_ru_power(ru1, |w| {
            w.set_tb_ru_power_2_low_bits(delta_10);
        });
        init.modify_tb_ru_power(ru1, |w| {
            w.set_tb_ru_power_3_low_bits(delta_10);
        });

        // Selectors 37..=40.
        init.modify_tb_ru_power(ru1, |w| {
            w.set_tb_ru_power_4_low_bits(delta_7);
        });
        let ru2 = element::<3>(2);
        init.modify_tb_ru_power(ru2, |w| {
            w.set_tb_ru_power_0_low_bits(delta_7);
        });
        init.modify_tb_ru_power(ru2, |w| {
            w.set_tb_ru_power_1_low_bits(delta_7);
        });
        init.modify_tb_ru_power(ru2, |w| {
            w.set_tb_ru_power_2_low_bits(delta_7);
        });

        // Selectors 53, 54 and 61.
        init.modify_tb_ru_power(ru2, |w| {
            w.set_tb_ru_power_3_low_bits(delta_4);
        });
        init.modify_tb_ru_power(ru2, |w| {
            w.set_tb_ru_power_4_low_bits(delta_4);
        });
        init.modify_tb_ru_power_tail(|w| {
            w.set_tail_power_index_low_bits(base);
        });
    }
}

#[cfg(test)]
mod tests;
