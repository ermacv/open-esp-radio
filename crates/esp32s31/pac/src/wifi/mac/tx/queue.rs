//! Safe generated-PAC transactions for ordinary EDCA TX queue ownership.

#![forbid(unsafe_code)]

use super::MacTxProtection;
use crate::{MacInterface, svd};

const ORDINARY_QUEUE_COUNT: u32 = 4;
const LAST_CONTROL_ADDRESS: u32 = 0x2010_4d70;
const CONTROL_STRIDE: u32 = 0x10;

/// Replace both software request flags while the prepared queue is idle.
///
/// The descriptor-bound preparation owner calls this after vector formatting,
/// before ENABLE|VALID. A single fresh-read update preserves spacing and
/// replaces any CTS/RTS request retained from a previous PHY or attempt.
#[inline(always)]
pub(crate) fn configure_protection(
    control: &svd::WifiMacTxQueueControl,
    queue: u32,
    protection: MacTxProtection,
) {
    control
        .protection(physical_bank(queue))
        .modify(|_, writer| {
            writer
                .software_cts()
                .bit(matches!(protection, MacTxProtection::CtsToSelf))
                .software_rts()
                .bit(matches!(protection, MacTxProtection::RtsCts))
        });
}

#[inline(always)]
const fn physical_bank(queue: u32) -> usize {
    assert!(
        queue < ORDINARY_QUEUE_COUNT,
        "ordinary TX queue is out of range"
    );
    (ORDINARY_QUEUE_COUNT - 1 - queue) as usize
}

#[inline(always)]
const fn control_address(queue: u32) -> u32 {
    assert!(
        queue < ORDINARY_QUEUE_COUNT,
        "ordinary TX queue is out of range"
    );
    LAST_CONTROL_ADDRESS - queue * CONTROL_STRIDE
}

#[inline(always)]
fn set_software_rts(registers: &svd::WifiMacTxQueueControl, queue: u32, enabled: bool) {
    registers
        .protection(physical_bank(queue))
        .modify(|_, writer| writer.software_rts().bit(enabled));
}

/// Program the recovered RTS register edge without publishing a queue.
///
/// Complete `hal_he_set_tx_protection`: the software request preserves CTS
/// and spacing. A supplied HE threshold replaces the threshold word after
/// that RMW; `None` leaves it untouched, including its disabled state.
/// The threshold is in bytes, converted from the HE duration threshold for
/// the selected PHY by the vendor's `ic_get_he_rts_threshold_bytes`. It is
/// not an RTS/CTS Duration/NAV value. The caller must own that conversion and
/// the protected exchange before using this edge in an on-air transaction.
#[inline(always)]
pub(crate) fn configure_rts(
    control: &svd::WifiMacTxQueueControl,
    vector: &svd::WifiMacTxQueueVector,
    queue: u32,
    enabled: bool,
    threshold_bytes: Option<u16>,
) {
    set_software_rts(control, queue, enabled);
    if let Some(threshold_bytes) = threshold_bytes {
        svd::zero_based_field_write::publish_mac_tx_rts_threshold(
            vector,
            physical_bank(queue),
            threshold_bytes,
            true,
        );
    }
}

/// Disable the HE threshold in logical queue order, retaining byte-threshold state.
///
/// Complete `hal_he_disable_rts_threshold`; this does not clear software RTS.
#[inline(always)]
pub(crate) fn disable_he_rts_threshold(vector: &svd::WifiMacTxQueueVector) {
    for queue in 0..ORDINARY_QUEUE_COUNT {
        vector
            .he_rts_control(physical_bank(queue))
            .modify(|_, writer| writer.he_rts_disabled().set_bit());
    }
}

#[inline(always)]
pub(crate) fn set_cca_force(registers: &svd::WifiMacTxCommon, value: u32) -> u32 {
    registers
        .cca_control()
        .modify(|_, writer| writer.force().set(value as u8));
    0
}

#[inline(always)]
pub(crate) fn completion_pending(registers: &svd::WifiMacTxCommon, queue: u8) -> bool {
    let state = registers.complete_state().read();
    match queue {
        0 => state.queue_0().bit_is_set(),
        1 => state.queue_1().bit_is_set(),
        2 => state.queue_2().bit_is_set(),
        3 => state.queue_3().bit_is_set(),
        _ => panic!("ordinary TX queue is out of range"),
    }
}

#[inline(always)]
pub(crate) fn collision_pending(registers: &svd::WifiMacTxCommon, queue: u8) -> bool {
    let state = registers.queue_state().read();
    match queue {
        0 => state.collision_queue_0().bit_is_set(),
        1 => state.collision_queue_1().bit_is_set(),
        2 => state.collision_queue_2().bit_is_set(),
        3 => state.collision_queue_3().bit_is_set(),
        _ => panic!("ordinary TX queue is out of range"),
    }
}

#[inline(always)]
pub(crate) fn timeout_pending(registers: &svd::WifiMacTxCommon, queue: u8) -> bool {
    let state = registers.queue_state().read();
    match queue {
        0 => state.timeout_queue_0().bit_is_set(),
        1 => state.timeout_queue_1().bit_is_set(),
        2 => state.timeout_queue_2().bit_is_set(),
        3 => state.timeout_queue_3().bit_is_set(),
        _ => panic!("ordinary TX queue is out of range"),
    }
}

#[inline(always)]
pub(crate) fn queue_in_trigger_flow(registers: &svd::WifiMacTxCommon, queue: u8) -> bool {
    let state = registers.queue_state().read();
    match queue {
        0 => state.trigger_flow_queue_0().bit_is_set(),
        1 => state.trigger_flow_queue_1().bit_is_set(),
        2 => state.trigger_flow_queue_2().bit_is_set(),
        3 => state.trigger_flow_queue_3().bit_is_set(),
        _ => panic!("ordinary TX queue is out of range"),
    }
}

/// Return the complete vendor ABI projection for the isolated comparison
/// probe. Production completion code selects one named queue field directly.
#[cfg(feature = "validation-probes")]
#[inline(always)]
pub(crate) fn validation_trigger_flow_state(registers: &svd::WifiMacTxCommon) -> u32 {
    let state = registers.queue_state().read();
    u32::from(state.trigger_flow_queue_0().bit_is_set())
        + u32::from(state.trigger_flow_queue_1().bit_is_set()) * 2
        + u32::from(state.trigger_flow_queue_2().bit_is_set()) * 4
        + u32::from(state.trigger_flow_queue_3().bit_is_set()) * 8
        + u32::from(state.bits_28_31_opaque().bits()) * 16
}

#[inline(always)]
pub(crate) fn acknowledge_completion(registers: &svd::WifiMacTxCommon, queue: u8) {
    assert!(queue < ORDINARY_QUEUE_COUNT as u8);
    let state = registers.complete_clear().read();
    svd::zero_based_field_write::acknowledge_mac_tx_completion(
        registers,
        state.queue_0().bit_is_set() || queue == 0,
        state.queue_1().bit_is_set() || queue == 1,
        state.queue_2().bit_is_set() || queue == 2,
        state.queue_3().bit_is_set() || queue == 3,
        state.bits_4_31_opaque().bits(),
    );
}

#[inline(always)]
pub(crate) fn acknowledge_collision(registers: &svd::WifiMacTxCommon, queue: u8) {
    assert!(queue < ORDINARY_QUEUE_COUNT as u8);
    svd::zero_based_field_write::clear_mac_tx_collision_state(
        registers,
        queue == 0,
        queue == 1,
        queue == 2,
        queue == 3,
    );
}

#[inline(always)]
pub(crate) fn acknowledge_timeout(registers: &svd::WifiMacTxCommon, queue: u8) {
    assert!(queue < ORDINARY_QUEUE_COUNT as u8);
    svd::zero_based_field_write::clear_mac_tx_timeout_state(
        registers,
        queue == 0,
        queue == 1,
        queue == 2,
        queue == 3,
    );
}

#[inline(always)]
pub(crate) fn queue_enabled(registers: &svd::WifiMacTxQueueControl, queue: u32) -> bool {
    registers
        .control(physical_bank(queue))
        .read()
        .enable()
        .bit()
}

#[inline(always)]
pub(crate) fn queue_valid(registers: &svd::WifiMacTxQueueControl, queue: u32) -> bool {
    registers.control(physical_bank(queue)).read().valid().bit()
}

#[inline(always)]
pub(crate) fn invalidate_queue(registers: &svd::WifiMacTxQueueControl, queue: u32) -> u32 {
    registers
        .control(physical_bank(queue))
        .modify(|_, writer| writer.valid().clear_bit());
    control_address(queue)
}

#[inline(always)]
pub(crate) fn disable_queue(registers: &svd::WifiMacTxQueueControl, queue: u32) -> u32 {
    registers
        .control(physical_bank(queue))
        .modify(|_, writer| writer.valid().clear_bit().enable().clear_bit());
    control_address(queue)
}

#[inline(always)]
pub(crate) fn publish_queue(registers: &svd::WifiMacTxQueueControl, queue: u32) -> u32 {
    registers
        .control(physical_bank(queue))
        .modify(|_, writer| writer.valid().set_bit().enable().set_bit());
    0
}

/// Publish the three independent EDCA fields for one ordinary logical queue.
///
/// SOURCE: complete `libpp.a[hal_mac_tx.o]::hal_mac_tx_config_edca`.
/// Each field is a separate fresh-read RMW edge in this exact order.
#[inline(always)]
pub(crate) fn configure_edca(
    registers: &svd::WifiMacTxQueueControl,
    queue: u32,
    aifsn: u8,
    contention_window: u16,
    interface: MacInterface,
) -> u32 {
    let config = registers.config(physical_bank(queue));
    config.modify(|_, writer| writer.aifsn().set(aifsn));
    config.modify(|_, writer| writer.contention_window().set(contention_window));
    config.modify(|_, writer| writer.interface().set(interface.bits() as u8));
    0
}

#[cfg(test)]
mod tests;
