//! Cold MAC preparation and runtime start.

use oer_esp32s31_pac::{MacInterruptMask, wifi_mac_core_enable::control::Bits4_7Opaque};
use oer_mmio::Bits;

use super::{MacInterruptSetup, MacPlacement, WifiRadioRegisters, interrupt};

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Hold all common MAC gates while the cold configuration has no RX ring.
    ///
    /// SOURCE: complete `hal_disable_mac`, BLOB_LIBPP_HAL_ENABLE_MAC; the
    /// cold runtime image is the vendor's full-word zero store
    /// (BLOB_LIBPP_HAL_DISABLE_MAC), written here as every field zero.
    pub fn hold_mac_runtime(&mut self) {
        self.mac.wifi_mac_core_enable.modify_control(|w| {
            w.set_bits_4_7_opaque(Bits4_7Opaque::Disabled);
        });
        oer_mmio::fence();
        // Initialize the cold runtime image while the core is held, then
        // retain its no-power-save stop through BASE and walker publication.
        self.mac.wifi_mac_control.modify_control(|w| {
            w.set_bits_0_11_opaque(Bits::constant::<0>())
                .set_no_retention_stop_request(false)
                .set_tx_block_stop_request_low(false)
                .set_power_save_tx_block(
                    oer_esp32s31_pac::wifi_mac_control::control::PowerSaveTxBlock::Run,
                )
                .set_tx_block_stop_request_high(Bits::constant::<0>())
                .set_bits_24_31_opaque(0);
        });
        self.request_mac_channel_stop_without_power_save();
    }

    /// Publish the cold interrupt policy without opening the MAC core.
    ///
    /// SOURCE: interrupt suffix of complete `hal_enable_mac`.
    pub fn prepare_mac_interrupts(
        &mut self,
        interrupts: &mut MacInterruptSetup<M>,
        event_mask: MacInterruptMask,
    ) {
        interrupt::publish_mac_interrupt_mask(&mut interrupts.mac, event_mask);
    }

    /// Clock the prepared MAC core without releasing its runtime stop.
    ///
    /// SOURCE: BLOB_LIBPP_HAL_ENABLE_MAC clears the four common gates in one
    /// fresh-read RMW. Runtime orders this edge before BASE while MAC RX
    /// remains held until the later live-walker resume.
    pub fn prepare_mac_runtime(&mut self) {
        self.mac.wifi_mac_core_enable.modify_control(|w| {
            w.set_bits_4_7_opaque(Bits4_7Opaque::Enabled);
        });
        oer_mmio::fence();
    }

    /// Whether every common MAC disable gate is still asserted.
    pub fn mac_core_gates_closed(&self) -> bool {
        self.mac
            .wifi_mac_core_enable
            .read_control()
            .bits_4_7_opaque_variant()
            == Some(Bits4_7Opaque::Disabled)
    }
}
