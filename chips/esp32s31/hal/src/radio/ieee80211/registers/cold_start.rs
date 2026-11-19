//! Register-local operations of the cold MAC handshake.

use super::{MacPlacement, WifiRadioRegisters};

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Set the cold-start request bit.
    pub fn request_mac_cold_start(&mut self) {
        self.mac
            .wifi_mac_cold_handshake
            .modify_cold_handshake_control(|w| {
                w.set_request(true);
            });
    }

    /// Sample the cold-start ready field once.
    pub fn sample_mac_cold_start_ready(&self) -> bool {
        self.mac
            .wifi_mac_cold_handshake
            .read_cold_handshake_control()
            .ready()
    }

    /// TEMPORARY experiment (`tasks/227` S2, probe branch only): the MAC
    /// words of `hal_deinit` before its interrupt edges, edge for edge: the
    /// four coexistence setters with zero, then `AUX_ENABLE` bit 0 set.
    ///
    /// SOURCE: pinned `libpp.a[hal_mac.o]::hal_deinit+0x0..0x42` (calls of
    /// `hal_coex_enable_default_pti`, `hal_set_rx_active_pti`,
    /// `hal_set_rx_ack_pti`, `hal_set_wifi_default_pti`, each with 0; then
    /// `AUX_ENABLE |= 1`), and the complete leaves
    /// `libpp.a[hal_coex.o]::hal_coex_enable_default_pti` (argument 0 clears
    /// `DEFAULT_CONTROL` bit 4) and `hal_set_rx_active_pti` (the low nibble
    /// of `RX_PTI`), read with Blobray.
    pub fn experiment_hal_deinit_words(&mut self) {
        let coex = &mut self.mac.wifi_mac_coex_init;
        coex.modify_default_control(|w| {
            w.set_default_pti_enable(false);
        });
        coex.modify_rx_pti(|w| {
            w.set_rx_active(oer_portable_mmio::Bits::constant::<0>());
        });
        coex.modify_rx_pti(|w| {
            w.set_rx_ack(oer_portable_mmio::Bits::constant::<0>());
        });
        coex.modify_default_control(|w| {
            w.set_wifi_default_pti(oer_portable_mmio::Bits::constant::<0>());
        });
        self.mac.wifi_mac_txrx_suffix.modify_aux_enable(|w| {
            w.set_hardware_statistics_clear_pulse(true);
        });
    }

    /// TEMPORARY experiment (`tasks/227` S2): `hal_deinit`'s last edge, the
    /// handshake's request set again.
    ///
    /// SOURCE: pinned `libpp.a[hal_mac.o]::hal_deinit+0x5e..0x68`
    /// (`COLD_HANDSHAKE_CONTROL |= 2`); its wait follows at `+0x6a..0x76`
    /// (bit 0 read until zero), which the caller bounds.
    pub fn experiment_hal_deinit_request(&mut self) {
        self.request_mac_cold_start();
    }
}
