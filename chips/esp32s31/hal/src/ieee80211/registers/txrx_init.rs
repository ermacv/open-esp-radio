//! The direct `mac_txrx_init` prefix, its HE callbacks and its suffix.

use oer_esp32s31_pac::SharedRadioRegisters;
use oer_mmio::Bits;

use super::{MacPlacement, WifiRadioRegisters, WifiRxDmaRegisters, bits, every_index};

/// The four receive queues' default words.
const RX_QUEUES: [oer_mmio::Index<4>; 4] = every_index();

/// Vendor MAC-delay jitter in the hardware's complete representable range.
/// This value survives the driver/HAL boundary without erasing its proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacDelaySlot(u8);

impl MacDelaySlot {
    pub const fn from_random(random: u32) -> Self {
        // The remainder is below eleven, so it fits a byte.
        Self((random % 11).to_le_bytes()[0])
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// The CCK ACK and CTS rate tables' vendor image: rate selector one for
/// 2 Mbit/s and five for 2 Mbit/s with short preamble, zero elsewhere.
///
/// SOURCE: BLOB_LIBPP_MAC_TXRX_CALLBACKS (`hal_he_set_ack_rate(0)` stores
/// `0x0005_0100` to both tables).
const CCK_RATE_2M: u8 = 0x01;
const CCK_RATE_2M_SHORT_PREAMBLE: u8 = 0x05;

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Apply all eighteen direct RMW edges before the first HE callback.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_txrx_init`,
    /// offsets `0x08..0xd0`. Callback effects and the direct suffix remain
    /// outside this deliberately bounded operation.
    pub fn initialize_mac_txrx_prefix(&mut self) {
        let init = &mut self.mac.wifi_mac_txrx_prefix;

        init.modify_feature_edges(|w| {
            w.set_bit_31_opaque(true)
                .set_bit_23_opaque(true)
                .set_bit_15_opaque(true)
                .set_bit_13_opaque(true);
        });
        init.modify_feature_edges(|w| {
            w.set_bit_12_opaque(true);
        });
        init.modify_feature_edges(|w| {
            w.set_bit_28_opaque(true);
        });
        init.modify_mode_control(|w| {
            w.set_bit_3_opaque(false);
        });

        let [queue_0, queue_1, _, _] = RX_QUEUES;
        for queue in RX_QUEUES {
            init.modify_rx_queue_default(queue, |w| {
                w.set_bits_16_23_opaque(0)
                    .set_bit_24_opaque(false)
                    .set_bit_25_opaque(false)
                    .set_bit_26_opaque(false)
                    .set_bits_27_31_opaque(Bits::constant::<0>());
            });
        }
        init.modify_rx_queue_default(queue_0, |w| {
            w.set_bit_24_opaque(true);
        });
        init.modify_rx_queue_default(queue_1, |w| {
            w.set_bit_24_opaque(true);
        });
        init.modify_rx_queue_default(queue_0, |w| {
            w.set_bit_26_opaque(true);
        });
        init.modify_rx_queue_default(queue_1, |w| {
            w.set_bit_26_opaque(true);
        });

        init.modify_feature_edges(|w| {
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
        init.modify_shared_enable_control(|w| {
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

        // The four rate-table words are whole-word stores in the vendor leaf;
        // each field is written, the vendor's zeros included.
        callbacks.modify_ack_rate_table(|w| {
            w.set_bpsk_rate(0x0b)
                .set_qpsk_rate(0x0a)
                .set_qam16_rate(0x09)
                .set_bits_24_31_opaque(0);
        });
        callbacks.modify_cts_rate_table(|w| {
            w.set_bpsk_rate(0x0b)
                .set_qpsk_rate(0x0a)
                .set_qam16_rate(0x09)
                .set_bits_24_31_opaque(0);
        });
        callbacks.modify_ack_cck_rate_table(|w| {
            w.set_rate_1m(0)
                .set_rate_2m(CCK_RATE_2M)
                .set_rate_2m_short_preamble(CCK_RATE_2M_SHORT_PREAMBLE)
                .set_bits_24_27_opaque(Bits::constant::<0>())
                .set_bit_30_opaque(false);
        });
        callbacks.modify_cts_cck_rate_table(|w| {
            w.set_rate_1m(0)
                .set_rate_2m(CCK_RATE_2M)
                .set_rate_2m_short_preamble(CCK_RATE_2M_SHORT_PREAMBLE);
        });
        callbacks.modify_bb_rx_hang_control(|w| {
            w.set_bits_0_11_opaque(Bits::constant::<0x00f>());
        });
    }

    /// Apply all nine direct RMW edges after the three HE callbacks.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_txrx_init`,
    /// offsets `0xee..0x16e`.
    pub fn initialize_mac_txrx_suffix(
        &mut self,
        shared: &mut SharedRadioRegisters,
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
        shared.shared_radio_init_control_mut().modify_control(|w| {
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
        rx_dma.dma.modify_rx_control(|w| {
            w.set_walker_enable(false);
        });
    }
}
