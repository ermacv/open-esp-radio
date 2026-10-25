//! Isolated probe-image bridges (`cfg(oer_probe)`).
//!
//! The one layer that adapts vendor-shaped scalar ABIs to the HAL's typed
//! operations for the vendor comparison's probe images. A probe that can
//! use the public API does so on the owners [`runtime_owner`] and
//! [`mac_interrupt_setup`] claim; the functions here exist only for state
//! the public API does not reach: raw vendor arguments and the PAC's
//! internal register transactions.

#![forbid(unsafe_code)]

use crate::{
    ieee80211::{dma::rx_dma::RxDma, station_wake::StationWakeState},
    owner::{MacInterruptRegisters, MacInterruptSetup, MacPowerInterruptRegisters, RadioAccess},
    types::{MacHeTxProgram, MacHtTxProgram, MacInterface, MacLegacyTxProgram, TxBlockAckPayload},
};

/// The running Wi-Fi register owner of a probe image, clocked and
/// initialized as far as the image's own sequence brought it.
pub fn runtime_owner() -> RadioAccess {
    RadioAccess::from_clocked(
        oer_esp32s31_pac::validation::wifi_radio_registers(),
        oer_esp32s31_pac::validation::wifi_rx_dma_registers(),
        StationWakeState::default(),
        true,
    )
}

/// The task-side MAC interrupt setup of a probe image.
pub fn mac_interrupt_setup() -> MacInterruptSetup {
    MacInterruptSetup {
        inner: oer_esp32s31_pac::validation::mac_interrupt_setup(),
    }
}

fn mac_interrupt_registers() -> MacInterruptRegisters {
    MacInterruptRegisters {
        inner: oer_esp32s31_pac::validation::mac_interrupt_registers(),
    }
}

fn mac_power_interrupt_registers() -> MacPowerInterruptRegisters {
    MacPowerInterruptRegisters {
        inner: oer_esp32s31_pac::validation::mac_power_interrupt_registers(),
    }
}

pub fn hal_get_sta_tsf(low: Option<&mut u32>, high: Option<&mut u32>) {
    runtime_owner().pac().validation_station_tsf(low, high);
}

pub fn hal_mac_interrupt_get_event() -> u32 {
    mac_interrupt_registers()
        .mac_interrupt_status()
        .probe_bits()
}

/// Acknowledge the events the MAC status holds now: a fresh sample of the
/// handler's part, never an image the comparison chose. Returns the
/// acknowledged bits; `events` (the vendor's argument) is not written.
pub fn hal_mac_interrupt_clr_event(_events: u32) -> u32 {
    let mut registers = mac_interrupt_registers();
    let snapshot = registers.mac_interrupt_status();
    let acknowledged = snapshot.probe_bits();
    registers.acknowledge_mac_interrupts(snapshot);
    acknowledged
}

pub fn hal_pwr_interrupt_get_event() -> u32 {
    mac_power_interrupt_registers()
        .power_interrupt_status()
        .probe_bits()
}

/// Acknowledge the WDEVPWR events the status holds now, as
/// [`hal_mac_interrupt_clr_event`] does for the MAC.
pub fn hal_pwr_interrupt_clr_event(_events: u32) -> u32 {
    let mut registers = mac_power_interrupt_registers();
    let snapshot = registers.power_interrupt_status();
    let acknowledged = snapshot.probe_bits();
    registers.acknowledge_power_interrupts(snapshot);
    acknowledged
}

pub fn hal_mac_rx_disable(passthrough: u32) -> u32 {
    runtime_owner()
        .rx_dma()
        .registers_for_probe()
        .validation_set_mac_rx_walker_enabled(false);
    passthrough
}

pub fn hal_mac_rx_enable(passthrough: u32) -> u32 {
    runtime_owner()
        .rx_dma()
        .registers_for_probe()
        .validation_set_mac_rx_walker_enabled(true);
    passthrough
}

pub fn hal_mac_rx_set_base(address: u32) -> u32 {
    runtime_owner()
        .rx_dma()
        .registers_for_probe()
        .validation_write_mac_rx_descriptor_base(address);
    address
}

pub fn hal_mac_rx_is_dscr_reload() -> u32 {
    u32::from(runtime_owner().rx_dma().reload_pending())
}

pub fn hal_mac_rx_set_dscr_reload(passthrough: u32) -> u32 {
    runtime_owner()
        .rx_dma()
        .registers_for_probe()
        .validation_request_mac_rx_descriptor_reload();
    passthrough
}

pub fn hal_mac_tx_set_cca(value: u32) -> u32 {
    runtime_owner().pac_mut().validation_set_mac_tx_cca(value)
}

pub fn hal_mac_get_txq_in_trig_flow_state() -> u32 {
    runtime_owner().pac().validation_mac_tx_trigger_flow_state()
}

pub fn hal_mac_is_txq_enabled(queue: u32) -> u32 {
    u32::from(runtime_owner().pac().validation_mac_tx_queue_enabled(queue))
}

pub fn hal_mac_is_txq_valid(queue: u32) -> u32 {
    u32::from(runtime_owner().pac().validation_mac_tx_queue_valid(queue))
}

pub fn hal_mac_set_txq_invalid(queue: u32) -> u32 {
    runtime_owner()
        .pac_mut()
        .validation_invalidate_mac_tx_queue(queue)
}

pub fn hal_mac_txq_disable(queue: u32) -> u32 {
    runtime_owner()
        .pac_mut()
        .validation_disable_mac_tx_queue(queue)
}

/// The recovered RTS request and optional HE byte-threshold publication,
/// without DMA.
pub fn configure_mac_tx_rts(queue: u32, enabled: bool, threshold_bytes: Option<u16>) {
    runtime_owner()
        .pac_mut()
        .validation_configure_mac_tx_rts(queue, enabled, threshold_bytes);
}

/// The production HE threshold disable; software RTS and the byte threshold
/// stay.
pub fn disable_mac_he_rts_threshold() {
    runtime_owner()
        .pac_mut()
        .validation_disable_mac_he_rts_threshold();
}

/// The production queue publication, without the vendor's access
/// bookkeeping.
pub fn ordinary_tx_publish(queue: u8) {
    runtime_owner()
        .pac_mut()
        .validation_start_prepared_mac_tx(queue);
}

/// The production completion acknowledgement; the probe supplies selector
/// two.
pub fn ordinary_tx_acknowledge_completion(queue: u8) {
    runtime_owner()
        .pac_mut()
        .validation_acknowledge_mac_tx_completion(queue);
}

pub fn hal_mac_tx_config_edca(
    queue: u32,
    aifsn: u8,
    contention_window: u16,
    interface: MacInterface,
) -> u32 {
    runtime_owner().pac_mut().validation_configure_mac_tx_edca(
        queue,
        aifsn,
        contention_window,
        interface,
    )
}

pub fn hal_mac_tx_set_ppdu(queue: u8, program: MacHtTxProgram) -> u32 {
    runtime_owner()
        .pac_mut()
        .validation_program_ht_mac_tx_ppdu(queue, program);
    0
}

pub fn hal_mac_tx_set_legacy_ppdu(queue: u8, program: MacLegacyTxProgram) -> u32 {
    runtime_owner()
        .pac_mut()
        .validation_program_legacy_mac_tx_ppdu(queue, program);
    0
}

pub fn hal_mac_tx_set_he_ppdu(queue: u8, program: MacHeTxProgram) -> u32 {
    runtime_owner()
        .pac_mut()
        .validation_program_he_mac_tx_ppdu(queue, program);
    0
}

pub fn hal_mac_tx_get_blockack(queue: u8) -> Option<TxBlockAckPayload> {
    runtime_owner().pac().read_tx_block_ack_payload(queue)
}

/// Compiled leaves of `hal_external_coexist.o` on the probe image's shared
/// registers.
pub mod coex_external {
    use oer_esp32s31_pac::{ExternalCoexPriority, ExternalCoexRole, ExternalCoexWires};

    /// A vendor argument outside the reviewed domain: no write.
    pub const REJECTED: u32 = u32::MAX;

    fn with_registers(operation: impl FnOnce(&mut oer_esp32s31_pac::SharedRadioRegisters)) -> u32 {
        operation(&mut oer_esp32s31_pac::validation::shared_radio_registers());
        0
    }

    fn role(mode: u32) -> Option<ExternalCoexRole> {
        match mode {
            0 => Some(ExternalCoexRole::Leader),
            2 => Some(ExternalCoexRole::Follower),
            _ => None,
        }
    }

    fn wires(wire: u32) -> Option<ExternalCoexWires> {
        match wire {
            0 => Some(ExternalCoexWires::One),
            1 => Some(ExternalCoexWires::Two),
            2 => Some(ExternalCoexWires::Three),
            3 => Some(ExternalCoexWires::Four),
            _ => None,
        }
    }

    fn priority(value: u32) -> Option<ExternalCoexPriority> {
        ExternalCoexPriority::new((value & u32::from(ExternalCoexPriority::MAX)) as u8)
    }

    /// `hal_set_extern_pti_mode(mode)` with the stored wire type.
    pub fn hal_set_extern_pti_mode(mode: u32, wire: u32) -> u32 {
        let (Some(role), Some(wires)) = (role(mode), wires(wire)) else {
            return REJECTED;
        };
        with_registers(|registers| registers.configure_external_coex_mode(role, wires))
    }

    /// `hal_set_extern_pti(first, second, third)` in the mode and with the
    /// wire type `hal_set_extern_pti_mode` stored.
    pub fn hal_set_extern_pti(mode: u32, wire: u32, first: u32, second: u32, third: u32) -> u32 {
        let (Some(role), Some(wires), Some(first), Some(second), Some(third)) = (
            role(mode),
            wires(wire),
            priority(first),
            priority(second),
            priority(third),
        ) else {
            return REJECTED;
        };
        with_registers(|registers| {
            registers.publish_external_coex_priorities(role, wires, [first, second, third])
        })
    }

    /// `hal_clr_extern_pti` in the stored mode.
    pub fn hal_clr_extern_pti(mode: u32) -> u32 {
        let Some(role) = role(mode) else {
            return REJECTED;
        };
        with_registers(|registers| registers.clear_external_coex_priorities(role))
    }

    /// `hal_enable_extern_coex` (`enabled`) or `hal_disable_extern_coex`.
    pub fn hal_set_extern_coex_enabled(enabled: bool) -> u32 {
        with_registers(|registers| registers.set_external_coex_enabled(enabled))
    }

    /// `ic_set_extern_coex_params` with the stored wire type: the work mode,
    /// then the grant delay and validity.
    pub fn ic_set_extern_coex_params(
        mode: u32,
        wire: u32,
        delay_us: u32,
        validate_high: bool,
    ) -> u32 {
        let (Some(role), Some(wires), Some(delay)) = (
            role(mode),
            wires(wire),
            oer_esp32s31_pac::ExternalCoexGrantDelay::new(delay_us),
        ) else {
            return REJECTED;
        };
        with_registers(|registers| {
            registers.configure_external_coex_mode(role, wires);
            registers.configure_external_coex_grant(delay, validate_high);
        })
    }
}
