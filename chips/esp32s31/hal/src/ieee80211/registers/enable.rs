//! Register-local cold MAC preparation and runtime start.

use oer_esp32s31_pac::wifi_mac::{control::PowerSaveTxBlock, core_enable_control::Bits4_7Opaque};
use oer_mmio::Bits;

use super::{MacInterruptMask, MacInterruptSetup, MacPlacement, WifiRadioRegisters};

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Hold all common MAC gates while the cold configuration has no RX ring.
    /// SOURCE: complete `hal_disable_mac`, BLOB_LIBPP_HAL_ENABLE_MAC.
    pub fn hold_mac_runtime(&mut self) {
        self.mac
            .wifi_mac_core_enable
            .modify_core_enable_control(|w| {
                w.set_bits_4_7_opaque(Bits4_7Opaque::Disabled);
            });
        oer_mmio::fence();
        // Initialize the cold runtime image while the core is held, then
        // retain its no-power-save stop through BASE and walker publication.
        self.open_mac_control_cold();
        self.request_mac_channel_stop_without_power_save();
    }

    /// The cold runtime image: `CONTROL` stored from zero.
    ///
    /// SOURCE: complete `hal_disable_mac`, BLOB_LIBPP_HAL_ENABLE_MAC. The
    /// vendor stores zero whole; every writable field is set to zero here,
    /// and the read-only `ACTIVE_STATE` lanes, which a store does not change,
    /// keep their value.
    fn open_mac_control_cold(&mut self) {
        self.mac.wifi_mac_control.modify_control(|w| {
            w.set_bits_0_11_opaque(Bits::constant::<0>())
                .set_no_retention_stop_request(false)
                .set_tx_block_stop_request_low(false)
                .set_power_save_tx_block(PowerSaveTxBlock::Run)
                .set_tx_block_stop_request_high(Bits::constant::<0>())
                .set_bits_24_31_opaque(0);
        });
    }

    /// Publish the cold interrupt policy without opening the MAC core.
    /// SOURCE: interrupt suffix of complete `hal_enable_mac`.
    pub fn prepare_mac_interrupts(
        &mut self,
        interrupts: &mut MacInterruptSetup<M>,
        event_mask: MacInterruptMask,
    ) {
        interrupts.publish_mac_interrupt_mask(event_mask);
    }

    /// Clock the prepared MAC core without releasing its runtime stop.
    /// SOURCE: BLOB_LIBPP_HAL_ENABLE_MAC. Runtime orders this edge before
    /// BASE while MAC RX remains held until the later live-walker resume.
    pub fn prepare_mac_runtime(&mut self) {
        self.mac
            .wifi_mac_core_enable
            .modify_core_enable_control(|w| {
                w.set_bits_4_7_opaque(Bits4_7Opaque::Enabled);
            });
        oer_mmio::fence();
    }

    /// Whether every common MAC disable gate is still asserted.
    pub fn mac_core_gates_closed(&self) -> bool {
        matches!(
            self.mac
                .wifi_mac_core_enable
                .read_core_enable_control()
                .bits_4_7_opaque_variant(),
            Some(Bits4_7Opaque::Disabled)
        )
    }
}

#[cfg(test)]
mod tests;
