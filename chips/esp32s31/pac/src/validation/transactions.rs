//! Register-local transactions of isolated probe images (`cfg(oer_probe)`).
//!
//! They never acquire a peripheral singleton and never expose a register
//! block: they let the HAL execute the same restricted PAC transactions
//! against an owner it already holds, with the vendor's scalar arguments.

use crate::{
    MacHeTxProgram, MacHtTxProgram, MacInterface, MacLegacyTxProgram, WifiRadioRegisters,
    WifiRxDmaRegisters,
};

impl WifiRadioRegisters {
    /// The recovered RTS control edge, without publishing a queue.
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

    /// The production HE threshold disable, in vendor queue order.
    pub fn validation_disable_mac_he_rts_threshold(&mut self) {
        crate::wifi::mac::tx::queue::disable_he_rts_threshold(
            &self.peripherals.wifi_mac.wifi_mac_tx_queue_vector,
        );
    }

    /// The production publication edge.
    pub fn validation_start_prepared_mac_tx(&mut self, queue: u8) {
        self.start_prepared_mac_tx(queue);
    }

    /// The production completion acknowledgement of one queue.
    pub fn validation_acknowledge_mac_tx_completion(&mut self, queue: u8) {
        crate::wifi::mac::tx::queue::acknowledge_completion(
            &self.peripherals.wifi_mac.wifi_mac_tx_common,
            queue,
        );
    }

    pub fn validation_station_tsf(&self, low: Option<&mut u32>, high: Option<&mut u32>) {
        crate::wifi::mac::tsf::snapshot_station_tsf(
            &self.peripherals.wifi_mac.wifi_mac_tsf_load,
            low,
            high,
        );
    }

    pub fn validation_set_mac_tx_cca(&mut self, value: u32) -> u32 {
        crate::wifi::mac::tx::queue::set_cca_force(
            &self.peripherals.wifi_mac.wifi_mac_tx_common,
            value,
        )
    }

    pub fn validation_mac_tx_trigger_flow_state(&self) -> u32 {
        crate::wifi::mac::tx::queue::trigger_flow_state_for_probe(
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

    /// The production HT queue programming, without forging a DMA
    /// publication capability.
    pub fn validation_program_ht_mac_tx_ppdu(&mut self, queue: u8, program: MacHtTxProgram) {
        self.program_ht_mac_tx_ppdu(queue, program);
    }

    /// The production legacy queue programming, without forging a DMA
    /// publication capability.
    pub fn validation_program_legacy_mac_tx_ppdu(
        &mut self,
        queue: u8,
        program: MacLegacyTxProgram,
    ) {
        self.program_legacy_mac_tx_ppdu(queue, program);
    }

    /// The production HE queue programming, without forging a DMA
    /// publication capability.
    pub fn validation_program_he_mac_tx_ppdu(&mut self, queue: u8, program: MacHeTxProgram) {
        self.program_he_mac_tx_ppdu(queue, program);
    }
}

impl WifiRxDmaRegisters {
    pub fn validation_set_mac_rx_walker_enabled(&mut self, enabled: bool) {
        crate::wifi::mac::rx::dma::set_walker_enabled(&self.peripherals.wifi_mac_rx_dma, enabled);
    }

    /// Write `address` to `RX_DESCRIPTOR_BASE` as the vendor's
    /// `hal_mac_rx_set_base` does.
    #[allow(
        unsafe_code,
        clippy::disallowed_methods,
        reason = "a probe image replays the vendor's raw base write"
    )]
    pub fn validation_write_mac_rx_descriptor_base(&mut self, address: u32) {
        // SAFETY: a probe image compares this register write with the
        // vendor's; it publishes no descriptor this image relies on, and
        // the probe enables the walker only on storage it placed itself.
        let head = unsafe {
            crate::layouts::DmaRef::new_unchecked(crate::layouts::DmaWindowAddress::new_unchecked(
                address,
            ))
        };
        crate::wifi::mac::rx::dma::write_descriptor_base(&self.peripherals.wifi_mac_rx_dma, head);
    }

    pub fn validation_request_mac_rx_descriptor_reload(&mut self) {
        crate::wifi::mac::rx::dma::request_descriptor_reload(&self.peripherals.wifi_mac_rx_dma);
    }
}
