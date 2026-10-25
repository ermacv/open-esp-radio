//! The direct `mac_txrx_init` transactions.

use oer_esp32s31_pac::wifi_mac::SharedRadioInitControl;
use oer_mmio::Bits;

use super::{MacPlacement, WifiRadioRegisters, WifiRxDmaRegisters, bits, element};

/// Vendor MAC-delay jitter in the hardware's complete representable range.
/// This value survives the driver/HAL boundary without erasing its proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacDelaySlot(u8);

impl MacDelaySlot {
    pub const fn from_random(random: u32) -> Self {
        Self((random % 11) as u8)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// The ACK and CTS OFDM rate tables `hal_he_set_ack_rate(0)` publishes:
/// BPSK, QPSK and 16-QAM response rates.
///
/// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]::hal_he_set_ack_rate`,
/// BLOB_LIBPP_MAC_TXRX_CALLBACKS.
const OFDM_RESPONSE_RATES: [u8; 3] = [0x0b, 0x0a, 0x09];

/// The ACK and CTS CCK rate tables `hal_he_set_ack_rate(0)` publishes
/// (`0x00050100`): 1M, 2M and short-preamble 2M.
///
/// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]::hal_he_set_ack_rate`,
/// BLOB_LIBPP_MAC_TXRX_CALLBACKS.
const CCK_RESPONSE_RATES: [u8; 3] = [0x00, 0x01, 0x05];

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Apply all eighteen direct RMW edges before the first HE callback.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_txrx_init`,
    /// offsets `0x08..0xd0`. Callback effects and the direct suffix remain
    /// outside this deliberately bounded operation.
    pub fn initialize_mac_txrx_prefix(&mut self) {
        let init = &mut self.mac.wifi_mac_txrx_prefix;

        init.modify_txrx_prefix_feature_edges(|w| {
            w.set_bit_31_opaque(true)
                .set_bit_23_opaque(true)
                .set_bit_15_opaque(true)
                .set_bit_13_opaque(true);
        });
        init.modify_txrx_prefix_feature_edges(|w| {
            w.set_bit_12_opaque(true);
        });
        init.modify_txrx_prefix_feature_edges(|w| {
            w.set_bit_28_opaque(true);
        });
        init.modify_mode_control(|w| {
            w.set_bit_3_opaque(false);
        });

        for queue in 0..4 {
            init.modify_rx_queue_default(element(queue), |w| {
                w.set_bits_16_23_opaque(0)
                    .set_bit_24_opaque(false)
                    .set_bit_25_opaque(false)
                    .set_bit_26_opaque(false)
                    .set_bits_27_31_opaque(Bits::constant::<0>());
            });
        }
        init.modify_rx_queue_default(element(0), |w| {
            w.set_bit_24_opaque(true);
        });
        init.modify_rx_queue_default(element(1), |w| {
            w.set_bit_24_opaque(true);
        });
        init.modify_rx_queue_default(element(0), |w| {
            w.set_bit_26_opaque(true);
        });
        init.modify_rx_queue_default(element(1), |w| {
            w.set_bit_26_opaque(true);
        });

        init.modify_txrx_prefix_feature_edges(|w| {
            w.set_bit_9_opaque(true);
        });
        init.modify_control_edges(|w| {
            w.set_bit_0_opaque(true);
        });
        init.modify_control_edges(|w| {
            w.set_bit_4_opaque(true);
        });
        init.modify_timing_control(|w| {
            w.set_bit_31_opaque(true);
        });
        init.modify_timing_control(|w| {
            w.set_bits_20_27_opaque(0x1b);
        });
        init.modify_txrx_prefix_shared_enable_control(|w| {
            w.set_bits_0_1_opaque(Bits::constant::<3>());
        });
    }

    /// Apply the on-chip paths of all three HE callbacks in `mac_txrx_init`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]`
    /// `hal_he_set_mac_delay`, `hal_he_set_ack_rate(0)` and
    /// `hal_he_set_bbrxhung_time(0)`. `delay_slot` is `_random() % 11`.
    ///
    /// The typed slot proves the complete vendor range `0..=10`.
    pub fn initialize_mac_txrx_callbacks(&mut self, delay_slot: MacDelaySlot) {
        let delay_slot = u32::from(delay_slot.value());

        let callbacks = &mut self.mac.wifi_mac_txrx_callbacks;
        callbacks.modify_delay_primary(|w| {
            w.set_rx_cck_delay(Bits::constant::<0x3b9>());
        });
        callbacks.modify_delay_primary(|w| {
            w.set_rx_ofdm_delay(bits(0xf5 + delay_slot));
        });
        callbacks.modify_delay_primary(|w| {
            w.set_tx_ofdm_delay(Bits::constant::<0x5e>());
        });
        callbacks.modify_delay_secondary(|w| {
            w.set_rx_11g_ofdm_delay(bits(0xfa + delay_slot));
        });
        callbacks.modify_delay_secondary(|w| {
            w.set_tx_cck_delay(Bits::constant::<0x276>());
        });

        callbacks.modify_ack_rate_table(|w| {
            w.set_bpsk_rate(
                oer_esp32s31_pac::wifi_mac::ack_rate_table::BpskRate::Ofdm6m.bits() as u8,
            )
            .set_qpsk_rate(
                oer_esp32s31_pac::wifi_mac::ack_rate_table::QpskRate::Ofdm12m.bits() as u8,
            )
            .set_qam16_rate(
                oer_esp32s31_pac::wifi_mac::ack_rate_table::Qam16Rate::Ofdm24m.bits() as u8,
            )
            .set_bits_24_31_opaque(0);
        });
        callbacks.modify_cts_rate_table(|w| {
            w.set_bpsk_rate(
                oer_esp32s31_pac::wifi_mac::cts_rate_table::BpskRate::Ofdm6m.bits() as u8,
            )
            .set_qpsk_rate(
                oer_esp32s31_pac::wifi_mac::cts_rate_table::QpskRate::Ofdm12m.bits() as u8,
            )
            .set_qam16_rate(
                oer_esp32s31_pac::wifi_mac::cts_rate_table::Qam16Rate::Ofdm24m.bits() as u8,
            )
            .set_bits_24_31_opaque(0);
        });
        callbacks.modify_ack_cck_rate_table(|w| {
            w.set_rate_1m(
                oer_esp32s31_pac::wifi_mac::ack_cck_rate_table::Rate1m::Dsss1m.bits() as u8,
            )
            .set_rate_2m(
                oer_esp32s31_pac::wifi_mac::ack_cck_rate_table::Rate2m::Dsss2m.bits() as u8,
            )
            .set_rate_2m_short_preamble(
                oer_esp32s31_pac::wifi_mac::ack_cck_rate_table::Rate2mShortPreamble::Dsss2mShort
                    .bits() as u8,
            )
            .set_bits_24_27_opaque(Bits::constant::<0>())
            .set_bit_30_opaque(false);
        });
        callbacks.modify_cts_cck_rate_table(|w| {
            w.set_rate_1m(
                oer_esp32s31_pac::wifi_mac::cts_cck_rate_table::Rate1m::Dsss1m.bits() as u8,
            )
            .set_rate_2m(
                oer_esp32s31_pac::wifi_mac::cts_cck_rate_table::Rate2m::Dsss2m.bits() as u8,
            )
            .set_rate_2m_short_preamble(
                oer_esp32s31_pac::wifi_mac::cts_cck_rate_table::Rate2mShortPreamble::Dsss2mShort
                    .bits() as u8,
            );
        });
        callbacks.modify_bb_rx_hang_control(|w| {
            w.set_bits_0_11_opaque(Bits::constant::<0x00f>());
        });
    }

    /// Apply all nine direct RMW edges after the three HE callbacks.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_txrx_init`,
    /// offsets `0xee..0x16e`. The shared radio initialization word belongs
    /// to the shared radio partition; its owner lends it.
    pub fn initialize_mac_txrx_suffix(
        &mut self,
        shared_init: &mut SharedRadioInitControl<M::Mac>,
        rx_dma: &mut WifiRxDmaRegisters<M>,
    ) {
        let callbacks = &mut self.mac.wifi_mac_txrx_callbacks;
        callbacks.modify_bb_rx_hang_control(|w| {
            w.set_bit_31_opaque(true);
        });
        callbacks.modify_bb_rx_hang_control(|w| {
            w.set_bit_30_opaque(true);
        });
        let init = &mut self.mac.wifi_mac_txrx_suffix;
        init.modify_default_image_a(|w| {
            w.set_bits_0_11_opaque(Bits::constant::<0x0f0>());
        });
        shared_init.modify_radio_init_control(|w| {
            w.set_bits_0_11_opaque(Bits::constant::<0x0f0>());
        });
        init.modify_field_control(|w| {
            w.set_bits_4_7_opaque(Bits::constant::<4>());
        });
        init.modify_gate_control(|w| {
            w.set_bits_16_30_opaque(Bits::constant::<0x7fff>());
        });
        init.modify_gate_control(|w| {
            w.set_bit_31_opaque(true);
        });
        init.modify_aux_enable(|w| {
            w.set_bit_1_opaque(true);
        });
        rx_dma.clear_mac_rx_walker_enable();
    }
}

#[cfg(test)]
mod tests;
