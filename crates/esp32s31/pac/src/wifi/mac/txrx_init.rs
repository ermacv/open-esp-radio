//! Generated-PAC ownership for the direct `mac_txrx_init` prefix.

#![forbid(unsafe_code)]

use crate::{SharedRadioRegisters, WifiRadioRegisters};

/// Vendor MAC-delay jitter in the hardware's complete representable range.
/// This value survives the driver/HAL/PAC boundary without erasing its proof.
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

impl WifiRadioRegisters {
    /// Apply all three ordered fresh-read updates of complete rev0 ROM
    /// `phy_sifs_reg_init`.
    pub fn initialize_phy_wifi_sifs(&mut self) {
        let callbacks = &self.peripherals.wifi_mac.wifi_mac_txrx_callbacks;
        callbacks
            .delay_secondary()
            .modify(|_, w| w.rx_11g_ofdm_delay().set(0xea));
        callbacks
            .delay_primary()
            .modify(|_, w| w.rx_cck_delay().set(0x3b8));
        callbacks
            .delay_primary()
            .modify(|_, w| w.rx_ofdm_delay().set(0xf0));
    }

    /// Apply all eighteen direct RMW edges before the first HE callback.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_txrx_init`,
    /// offsets `0x08..0xd0`. Callback effects and the direct suffix remain
    /// outside this deliberately bounded operation.
    pub fn initialize_mac_txrx_prefix(&mut self) {
        let init = &self.peripherals.wifi_mac.wifi_mac_txrx_prefix;

        init.feature_edges().modify(|_, w| {
            w.bit_31_opaque()
                .set_bit()
                .bit_23_opaque()
                .set_bit()
                .bit_15_opaque()
                .set_bit()
                .bit_13_opaque()
                .set_bit()
        });
        init.feature_edges()
            .modify(|_, w| w.bit_12_opaque().set_bit());
        init.feature_edges()
            .modify(|_, w| w.bit_28_opaque().set_bit());
        init.mode_control()
            .modify(|_, w| w.bit_3_opaque().clear_bit());

        for queue in 0..4 {
            init.rx_queue_default(queue).modify(|_, w| {
                w.bits_16_23_opaque()
                    .set(0)
                    .bit_24_opaque()
                    .clear_bit()
                    .bit_25_opaque()
                    .clear_bit()
                    .bit_26_opaque()
                    .clear_bit()
                    .bits_27_31_opaque()
                    .set(0)
            });
        }
        init.rx_queue_default(0)
            .modify(|_, w| w.bit_24_opaque().set_bit());
        init.rx_queue_default(1)
            .modify(|_, w| w.bit_24_opaque().set_bit());
        init.rx_queue_default(0)
            .modify(|_, w| w.bit_26_opaque().set_bit());
        init.rx_queue_default(1)
            .modify(|_, w| w.bit_26_opaque().set_bit());

        init.feature_edges()
            .modify(|_, w| w.bit_9_opaque().set_bit());
        init.control_edges()
            .modify(|_, w| w.bit_0_opaque().set_bit());
        init.control_edges()
            .modify(|_, w| w.bit_4_opaque().set_bit());
        init.timing_control()
            .modify(|_, w| w.bit_31_opaque().set_bit());
        init.timing_control()
            .modify(|_, w| w.bits_20_27_opaque().set(0x1b));
        init.shared_enable_control()
            .modify(|_, w| w.bits_0_1_opaque().set(3));
    }

    /// Apply the on-chip paths of all three HE callbacks in `mac_txrx_init`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]`
    /// `hal_he_set_mac_delay`, `hal_he_set_ack_rate(0)` and
    /// `hal_he_set_bbrxhung_time(0)`. `delay_slot` is `_random() % 11`.
    ///
    /// The typed slot proves the complete vendor range `0..=10`.
    pub fn initialize_mac_txrx_callbacks(&mut self, delay_slot: MacDelaySlot) {
        let delay_slot = delay_slot.value();

        let callbacks = &self.peripherals.wifi_mac.wifi_mac_txrx_callbacks;
        callbacks
            .delay_primary()
            .modify(|_, w| w.rx_cck_delay().set(0x3b9));
        callbacks
            .delay_primary()
            .modify(|_, w| w.rx_ofdm_delay().set(0xf5 + u16::from(delay_slot)));
        callbacks
            .delay_primary()
            .modify(|_, w| w.tx_ofdm_delay().set(0x5e));
        callbacks
            .delay_secondary()
            .modify(|_, w| w.rx_11g_ofdm_delay().set(0xfa + u16::from(delay_slot)));
        callbacks
            .delay_secondary()
            .modify(|_, w| w.tx_cck_delay().set(0x276));

        crate::svd::zero_based_field_write::mac_ack_rate_table(callbacks, 0x0b, 0x0a, 0x09, 0);
        crate::svd::zero_based_field_write::mac_cts_rate_table(callbacks, 0x0b, 0x0a, 0x09, 0);
        crate::svd::zero_based_field_write::mac_ack_cck_rate_table(callbacks, 0, 1, 5, 0);
        crate::svd::zero_based_field_write::mac_cts_cck_rate_table(callbacks, 0, 1, 5, 0);
        callbacks
            .bb_rx_hang_control()
            .modify(|_, w| w.bits_0_11_opaque().set(0x00f));
    }

    /// Apply all nine direct RMW edges after the three HE callbacks.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_txrx_init`,
    /// offsets `0xee..0x16e`.
    pub fn initialize_mac_txrx_suffix(&mut self, shared: &mut SharedRadioRegisters) {
        let callbacks = &self.peripherals.wifi_mac.wifi_mac_txrx_callbacks;
        callbacks
            .bb_rx_hang_control()
            .modify(|_, w| w.bit_31_opaque().set_bit());
        callbacks
            .bb_rx_hang_control()
            .modify(|_, w| w.bit_30_opaque().set_bit());
        let init = &self.peripherals.wifi_mac.wifi_mac_txrx_suffix;
        init.default_image_a()
            .modify(|_, w| w.bits_0_11_opaque().set(0x0f0));
        shared
            .shared_radio
            .shared_radio_init_control
            .control()
            .modify(|_, w| w.bits_0_11_opaque().set(0x0f0));
        init.field_control()
            .modify(|_, w| w.bits_4_7_opaque().set(4));
        init.gate_control()
            .modify(|_, w| w.bits_16_30_opaque().set(0x7fff));
        init.gate_control()
            .modify(|_, w| w.bit_31_opaque().set_bit());
        init.aux_enable().modify(|_, w| w.bit_1_opaque().set_bit());
        self.peripherals
            .wifi_mac
            .wifi_mac_rx_dma
            .rx_control()
            .modify(|_, w| w.walker_enable().clear_bit());
    }
}
