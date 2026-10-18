//! Register-local transactions used by isolated compiled probe images.
//!
//! This module never acquires a peripheral singleton and never exposes a
//! register block.  It only lets the HAL execute the same restricted PAC
//! transactions against an owner that the HAL already holds.

#![forbid(unsafe_code)]

use crate::{MacHeTxProgram, MacHtTxProgram, MacInterface, MacLegacyTxProgram, WifiRadioRegisters};

impl WifiRadioRegisters {
    /// Execute the production cold-init CTS reset for all ordinary queues.
    pub fn validation_initialize_mac_software_cts(&mut self) {
        self.initialize_mac_software_cts();
    }

    /// Execute the recovered RTS control edge without publishing a queue.
    pub fn validation_configure_mac_tx_rts(
        &mut self,
        queue: u32,
        enabled: bool,
        threshold_bytes: Option<u16>,
    ) {
        crate::wifi::mac::tx::queue::configure_rts(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_control,
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_vector,
            queue,
            enabled,
            threshold_bytes,
        );
    }

    /// Execute the production HE threshold disable in vendor queue order.
    pub fn validation_disable_mac_he_rts_threshold(&mut self) {
        crate::wifi::mac::tx::queue::disable_he_rts_threshold(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_vector,
        );
    }

    /// Execute the production publication edge in an isolated probe image.
    pub fn validation_start_prepared_mac_tx(&mut self, queue: u8) {
        self.start_prepared_mac_tx(queue);
    }

    /// Execute the production completion acknowledgement for one queue.
    pub fn validation_acknowledge_mac_tx_completion(&mut self, queue: u8) {
        crate::wifi::mac::tx::queue::acknowledge_completion(
            &self.peripherals.wifi_mac.wifi_mac_tx_common,
            queue,
        );
    }

    pub fn validation_station_tsf(&self, low: Option<&mut u32>, high: Option<&mut u32>) {
        crate::wifi::mac::tsf::snapshot_station_tsf(
            &self.peripherals.wifi_mac.wifi_mac_sta_tsf_load,
            low,
            high,
        );
    }

    pub fn validation_set_mac_rx_walker_enabled(&mut self, enabled: bool) {
        crate::wifi::mac::rx::dma::set_walker_enabled(
            &self.peripherals.wifi_mac.wifi_mac_rx_dma,
            enabled,
        );
    }

    pub fn validation_write_mac_rx_descriptor_base(&mut self, address: u32) {
        crate::wifi::mac::rx::dma::write_descriptor_base(
            &self.peripherals.wifi_mac.wifi_mac_rx_dma,
            address,
        );
    }

    pub fn validation_request_mac_rx_descriptor_reload(&mut self) {
        crate::wifi::mac::rx::dma::request_descriptor_reload(
            &self.peripherals.wifi_mac.wifi_mac_rx_dma,
        );
    }

    pub fn validation_set_mac_tx_cca(&mut self, value: u32) -> u32 {
        crate::wifi::mac::tx::queue::set_cca_force(
            &self.peripherals.wifi_mac.wifi_mac_tx_common,
            value,
        )
    }

    pub fn validation_mac_tx_trigger_flow_state(&self) -> u32 {
        crate::wifi::mac::tx::queue::validation_trigger_flow_state(
            &self.peripherals.wifi_mac.wifi_mac_tx_common,
        )
    }

    pub fn validation_mac_tx_queue_enabled(&self, queue: u32) -> bool {
        crate::wifi::mac::tx::queue::queue_enabled(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_control,
            queue,
        )
    }

    pub fn validation_mac_tx_queue_valid(&self, queue: u32) -> bool {
        crate::wifi::mac::tx::queue::queue_valid(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_control,
            queue,
        )
    }

    pub fn validation_invalidate_mac_tx_queue(&mut self, queue: u32) -> u32 {
        crate::wifi::mac::tx::queue::invalidate_queue(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_control,
            queue,
        )
    }

    pub fn validation_disable_mac_tx_queue(&mut self, queue: u32) -> u32 {
        crate::wifi::mac::tx::queue::disable_queue(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_control,
            queue,
        )
    }

    pub fn validation_configure_mac_tx_edca(
        &mut self,
        queue: u32,
        aifsn: u8,
        contention_window: u16,
        interface: MacInterface,
    ) -> u32 {
        crate::wifi::mac::tx::queue::configure_edca(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_control,
            queue,
            aifsn,
            contention_window,
            interface,
        )
    }

    /// Execute the exact production HT queue-programming transaction without
    /// forging a DMA publication capability in an isolated comparison image.
    pub fn validation_program_ht_mac_tx_ppdu(&mut self, queue: u8, program: MacHtTxProgram) {
        self.program_ht_mac_tx_ppdu(queue, program);
    }

    /// Execute the exact production legacy queue-programming transaction
    /// without forging a DMA publication capability.
    pub fn validation_program_legacy_mac_tx_ppdu(
        &mut self,
        queue: u8,
        program: MacLegacyTxProgram,
    ) {
        self.program_legacy_mac_tx_ppdu(queue, program);
    }

    /// Execute the exact production HE queue-programming transaction
    /// without forging a DMA publication capability.
    pub fn validation_program_he_mac_tx_ppdu(&mut self, queue: u8, program: MacHeTxProgram) {
        self.program_he_mac_tx_ppdu(queue, program);
    }
}
