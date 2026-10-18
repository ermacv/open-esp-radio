//! Isolated-image bridges for compiled production probes.
//!
//! This is the only layer that adapts vendor-shaped scalar ABIs to typed HAL
//! operations.  The PAC validation module only constructs finite owners; it
//! does not own these names, sequences, or semantic conversions.

#![forbid(unsafe_code)]

use crate::{
    owner::{
        RadioRuntimeOwner, validation_mac_interrupt_registers, validation_mac_interrupt_setup,
        validation_mac_power_interrupt_registers,
    },
    types::{
        MacHeTxProgram, MacHtTxProgram, MacInterface, MacInterruptSnapshot, MacLegacyTxProgram,
        MacPowerInterruptSnapshot, TxBlockAckPayload,
    },
};

#[inline(always)]
fn owner() -> RadioRuntimeOwner {
    RadioRuntimeOwner::claim_for_validation()
}

#[inline(always)]
pub fn hal_get_sta_tsf(low: Option<&mut u32>, high: Option<&mut u32>) {
    owner().pac().validation_station_tsf(low, high);
}

#[inline(always)]
pub fn hal_mac_interrupt_get_event() -> u32 {
    validation_mac_interrupt_registers()
        .mac_interrupt_status()
        .validation_bits()
}

#[inline(always)]
pub fn hal_mac_interrupt_clr_event(events: u32) -> u32 {
    let mut interrupts = validation_mac_interrupt_registers();
    interrupts.acknowledge_mac_interrupts(MacInterruptSnapshot::for_validation(events));
    events
}

#[inline(always)]
pub fn hal_disable_sta_beacon_filter() {
    let mut owner = owner();
    let mut setup = validation_mac_interrupt_setup();
    let _ = setup.prepare_connected_sta_without_power_save(&mut owner);
}

#[inline(always)]
pub fn hal_pwr_interrupt_get_event() -> u32 {
    validation_mac_power_interrupt_registers()
        .power_interrupt_status()
        .validation_bits()
}

#[inline(always)]
pub fn hal_pwr_interrupt_clr_event(events: u32) -> u32 {
    let mut interrupts = validation_mac_power_interrupt_registers();
    interrupts.acknowledge_power_interrupts(MacPowerInterruptSnapshot::for_validation(events));
    events
}

#[inline(always)]
pub fn hal_mac_rx_disable(passthrough: u32) -> u32 {
    owner()
        .pac_mut()
        .validation_set_mac_rx_walker_enabled(false);
    passthrough
}

#[inline(always)]
pub fn hal_mac_rx_enable(passthrough: u32) -> u32 {
    owner().pac_mut().validation_set_mac_rx_walker_enabled(true);
    passthrough
}

#[inline(always)]
pub fn hal_mac_rx_set_base(address: u32) -> u32 {
    owner()
        .pac_mut()
        .validation_write_mac_rx_descriptor_base(address);
    address
}

#[inline(always)]
pub fn hal_mac_rx_is_dscr_reload() -> u32 {
    u32::from(owner().pac().mac_rx_reload_pending())
}

#[inline(always)]
pub fn hal_mac_rx_set_dscr_reload(passthrough: u32) -> u32 {
    owner()
        .pac_mut()
        .validation_request_mac_rx_descriptor_reload();
    passthrough
}

#[inline(always)]
pub fn hal_mac_tx_set_cca(value: u32) -> u32 {
    owner().pac_mut().validation_set_mac_tx_cca(value)
}

#[inline(always)]
pub fn hal_mac_get_txq_in_trig_flow_state() -> u32 {
    owner().pac().validation_mac_tx_trigger_flow_state()
}

#[inline(always)]
pub fn hal_mac_is_txq_enabled(queue: u32) -> u32 {
    u32::from(owner().pac().validation_mac_tx_queue_enabled(queue))
}

#[inline(always)]
pub fn hal_mac_is_txq_valid(queue: u32) -> u32 {
    u32::from(owner().pac().validation_mac_tx_queue_valid(queue))
}

#[inline(always)]
pub fn hal_mac_set_txq_invalid(queue: u32) -> u32 {
    owner().pac_mut().validation_invalidate_mac_tx_queue(queue)
}

#[inline(always)]
pub fn hal_mac_txq_disable(queue: u32) -> u32 {
    owner().pac_mut().validation_disable_mac_tx_queue(queue)
}

/// Production cold-init CTS reset for all ordinary queues.
#[inline(always)]
pub fn initialize_mac_software_cts() {
    owner().pac_mut().validation_initialize_mac_software_cts();
}

/// Production RTS-clear edge used by ordinary queue preparation.
#[inline(always)]
pub fn ordinary_tx_clear_software_rts(queue: u32) {
    configure_mac_tx_rts(queue, false, None);
}

/// Recovered RTS request and optional HE byte-threshold publication, without DMA.
#[inline(always)]
pub fn configure_mac_tx_rts(queue: u32, enabled: bool, threshold_bytes: Option<u16>) {
    owner()
        .pac_mut()
        .validation_configure_mac_tx_rts(queue, enabled, threshold_bytes);
}

/// Production HE threshold disable; retains software RTS and byte-threshold state.
#[inline(always)]
pub fn disable_mac_he_rts_threshold() {
    owner().pac_mut().validation_disable_mac_he_rts_threshold();
}

/// Production queue publication, excluding vendor access bookkeeping.
#[inline(always)]
pub fn ordinary_tx_publish(queue: u8) {
    owner().pac_mut().validation_start_prepared_mac_tx(queue);
}

/// Production completion acknowledgement; the probe supplies selector two.
#[inline(always)]
pub fn ordinary_tx_acknowledge_completion(queue: u8) {
    owner()
        .pac_mut()
        .validation_acknowledge_mac_tx_completion(queue);
}

#[inline(always)]
pub fn hal_mac_tx_config_edca(
    queue: u32,
    aifsn: u8,
    contention_window: u16,
    interface: MacInterface,
) -> u32 {
    owner()
        .pac_mut()
        .validation_configure_mac_tx_edca(queue, aifsn, contention_window, interface)
}

#[inline(always)]
pub fn hal_mac_tx_set_ppdu(queue: u8, program: MacHtTxProgram) -> u32 {
    owner()
        .pac_mut()
        .validation_program_ht_mac_tx_ppdu(queue, program);
    0
}

#[inline(always)]
pub fn hal_mac_tx_set_legacy_ppdu(queue: u8, program: MacLegacyTxProgram) -> u32 {
    owner()
        .pac_mut()
        .validation_program_legacy_mac_tx_ppdu(queue, program);
    0
}

#[inline(always)]
pub fn hal_mac_tx_set_he_ppdu(queue: u8, program: MacHeTxProgram) -> u32 {
    owner()
        .pac_mut()
        .validation_program_he_mac_tx_ppdu(queue, program);
    0
}

#[inline(always)]
pub fn hal_mac_tx_get_blockack(queue: u8) -> Option<TxBlockAckPayload> {
    owner().pac().read_tx_block_ack_payload(queue)
}

#[inline(always)]
fn mac_interface(interface: u32) -> MacInterface {
    match interface {
        0 => MacInterface::Station,
        1 => MacInterface::AccessPoint,
        2 => MacInterface::Context2,
        3 => MacInterface::Context3,
        _ => panic!("verification MAC interface is out of range"),
    }
}

#[inline(always)]
pub fn hal_mac_set_addr(interface: u32, address: &[u8; 6]) {
    owner()
        .wifi_mac_hal()
        .program_interface_address(mac_interface(interface), *address);
}

#[inline(always)]
pub fn hal_mac_set_bssid(interface: u32, address: &[u8; 6]) {
    owner()
        .wifi_mac_hal()
        .program_interface_bssid(mac_interface(interface), *address);
}

/// Compiled leaves of `hal_external_coexist.o` on isolated shared registers.
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

    fn priority(value: u32) -> ExternalCoexPriority {
        ExternalCoexPriority::new((value & 0xf) as u8)
            .unwrap_or_else(|| unreachable!("a masked nibble is a priority"))
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
        let (Some(role), Some(wires)) = (role(mode), wires(wire)) else {
            return REJECTED;
        };
        with_registers(|registers| {
            registers.publish_external_coex_priorities(
                role,
                wires,
                [priority(first), priority(second), priority(third)],
            )
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
        let (Some(role), Some(wires)) = (role(mode), wires(wire)) else {
            return REJECTED;
        };
        with_registers(|registers| {
            registers.configure_external_coex_mode(role, wires);
            registers.configure_external_coex_grant(delay_us as u8, validate_high);
        })
    }
}
