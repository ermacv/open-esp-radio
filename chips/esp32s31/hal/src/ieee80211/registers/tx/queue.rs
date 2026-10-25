//! The register leaves of ordinary EDCA TX queue ownership.
//!
//! Logical queue `q` lives in physical bank `3 - q` of the queue-control and
//! queue-vector arrays; every leaf here takes the logical queue.

use oer_esp32s31_pac::wifi_mac::{
    AcknowledgeMacTxCompletionConsumed, WifiMacTxCommon, WifiMacTxQueueControl,
    WifiMacTxQueueVector,
};
use oer_mmio::{Index, Placement};

use super::MacTxProtection;
use crate::ieee80211::registers::{
    MacInterface, ORDINARY_QUEUE_COUNT, bits, element, ordinary_bank,
};

/// The physical bank of ordinary logical queue `queue`.
#[inline(always)]
pub(crate) const fn physical_bank(queue: u8) -> usize {
    assert!(
        queue < ORDINARY_QUEUE_COUNT,
        "ordinary TX queue is out of range"
    );
    (ORDINARY_QUEUE_COUNT - 1 - queue) as usize
}

/// The register-array element of ordinary logical queue `queue`.
#[inline(always)]
pub(crate) const fn bank(queue: u8) -> Index<4> {
    match ordinary_bank(queue) {
        Some(bank) => bank,
        None => panic!("ordinary TX queue is out of range"),
    }
}

/// The element of the eight-entry HE queue-control array that belongs to
/// ordinary logical queue `queue`: the upper four entries.
#[inline(always)]
pub(crate) const fn he_queue_control(queue: u8) -> Index<8> {
    element::<8>(4 + physical_bank(queue))
}

/// Replace both software request flags while the prepared queue is idle.
///
/// The descriptor-bound preparation owner calls this after vector formatting,
/// before ENABLE|VALID. A single fresh-read update preserves spacing and
/// replaces any CTS/RTS request retained from a previous PHY or attempt.
#[inline(always)]
pub(crate) fn configure_protection<P: Placement>(
    control: &mut WifiMacTxQueueControl<P>,
    queue: u8,
    protection: MacTxProtection,
) {
    control.modify_protection(bank(queue), |writer| {
        writer
            .set_software_cts(matches!(protection, MacTxProtection::CtsToSelf))
            .set_software_rts(matches!(protection, MacTxProtection::RtsCts));
    });
}

#[inline(always)]
fn set_software_rts<P: Placement>(
    registers: &mut WifiMacTxQueueControl<P>,
    queue: u8,
    enabled: bool,
) {
    registers.modify_protection(bank(queue), |writer| {
        writer.set_software_rts(enabled);
    });
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
pub(crate) fn configure_rts<P: Placement>(
    control: &mut WifiMacTxQueueControl<P>,
    vector: &mut WifiMacTxQueueVector<P>,
    queue: u8,
    enabled: bool,
    threshold_bytes: Option<u16>,
) {
    set_software_rts(control, queue, enabled);
    if let Some(threshold_bytes) = threshold_bytes {
        // The vendor's store from zero (`publish_mac_tx_rts_threshold`):
        // every declared field set, the disable bit zero.
        vector.modify_he_rts_control(bank(queue), |writer| {
            writer
                .set_rts_threshold_bytes(threshold_bytes)
                .set_rts_threshold_valid(true)
                .set_he_rts_disabled(false);
        });
    }
}

/// Disable the HE threshold in logical queue order, retaining byte-threshold state.
///
/// Complete `hal_he_disable_rts_threshold`; this does not clear software RTS.
#[inline(always)]
pub(crate) fn disable_he_rts_threshold<P: Placement>(vector: &mut WifiMacTxQueueVector<P>) {
    for queue in 0..ORDINARY_QUEUE_COUNT {
        vector.modify_he_rts_control(bank(queue), |writer| {
            writer.set_he_rts_disabled(true);
        });
    }
}

#[inline(always)]
pub(crate) fn set_cca_force<P: Placement>(registers: &mut WifiMacTxCommon<P>, value: u8) -> u32 {
    registers.modify_cca_control(|writer| {
        writer.set_force(bits(u32::from(value)));
    });
    0
}

#[inline(always)]
pub(crate) fn completion_pending<P: Placement>(registers: &WifiMacTxCommon<P>, queue: u8) -> bool {
    let state = registers.read_complete_state();
    match queue {
        0 => state.queue_0(),
        1 => state.queue_1(),
        2 => state.queue_2(),
        3 => state.queue_3(),
        _ => panic!("ordinary TX queue is out of range"),
    }
}

#[inline(always)]
pub(crate) fn collision_pending<P: Placement>(registers: &WifiMacTxCommon<P>, queue: u8) -> bool {
    let state = registers.read_queue_state();
    match queue {
        0 => state.collision_queue_0(),
        1 => state.collision_queue_1(),
        2 => state.collision_queue_2(),
        3 => state.collision_queue_3(),
        _ => panic!("ordinary TX queue is out of range"),
    }
}

#[inline(always)]
pub(crate) fn timeout_pending<P: Placement>(registers: &WifiMacTxCommon<P>, queue: u8) -> bool {
    let state = registers.read_queue_state();
    match queue {
        0 => state.timeout_queue_0(),
        1 => state.timeout_queue_1(),
        2 => state.timeout_queue_2(),
        3 => state.timeout_queue_3(),
        _ => panic!("ordinary TX queue is out of range"),
    }
}

#[inline(always)]
pub(crate) fn queue_in_trigger_flow<P: Placement>(
    registers: &WifiMacTxCommon<P>,
    queue: u8,
) -> bool {
    let state = registers.read_queue_state();
    match queue {
        0 => state.trigger_flow_queue_0(),
        1 => state.trigger_flow_queue_1(),
        2 => state.trigger_flow_queue_2(),
        3 => state.trigger_flow_queue_3(),
        _ => panic!("ordinary TX queue is out of range"),
    }
}

/// The completion event of ordinary logical queue `queue`.
const fn completion_event(queue: u8) -> AcknowledgeMacTxCompletionConsumed {
    match queue {
        0 => AcknowledgeMacTxCompletionConsumed::Queue0,
        1 => AcknowledgeMacTxCompletionConsumed::Queue1,
        2 => AcknowledgeMacTxCompletionConsumed::Queue2,
        3 => AcknowledgeMacTxCompletionConsumed::Queue3,
        _ => panic!("ordinary TX queue is out of range"),
    }
}

/// Acknowledge the completion of `queue` with every event the clear word
/// holds now.
///
/// SOURCE: BLOB_LIBPP_HAL_MAC_TX, PROMOTED_LMAC_TX: the vendor reads
/// `COMPLETE_CLEAR` and writes it back with the completed queue's bit set.
#[inline(always)]
pub(crate) fn acknowledge_completion<P: Placement>(registers: &mut WifiMacTxCommon<P>, queue: u8) {
    let consumed = completion_event(queue);
    let sampled = registers.sample_acknowledge_mac_tx_completion();
    registers.acknowledge_acknowledge_mac_tx_completion(sampled, Some(consumed));
}

#[inline(always)]
pub(crate) fn acknowledge_collision<P: Placement>(registers: &mut WifiMacTxCommon<P>, queue: u8) {
    match queue {
        0 => registers.trigger_queue_state_clear_collision_queue_0(),
        1 => registers.trigger_queue_state_clear_collision_queue_1(),
        2 => registers.trigger_queue_state_clear_collision_queue_2(),
        3 => registers.trigger_queue_state_clear_collision_queue_3(),
        _ => panic!("ordinary TX queue is out of range"),
    }
}

#[inline(always)]
pub(crate) fn acknowledge_timeout<P: Placement>(registers: &mut WifiMacTxCommon<P>, queue: u8) {
    match queue {
        0 => registers.trigger_queue_state_clear_timeout_queue_0(),
        1 => registers.trigger_queue_state_clear_timeout_queue_1(),
        2 => registers.trigger_queue_state_clear_timeout_queue_2(),
        3 => registers.trigger_queue_state_clear_timeout_queue_3(),
        _ => panic!("ordinary TX queue is out of range"),
    }
}

#[inline(always)]
pub(crate) fn queue_enabled<P: Placement>(registers: &WifiMacTxQueueControl<P>, queue: u8) -> bool {
    registers.read_control_n(bank(queue)).enable()
}

#[inline(always)]
pub(crate) fn queue_valid<P: Placement>(registers: &WifiMacTxQueueControl<P>, queue: u8) -> bool {
    registers.read_control_n(bank(queue)).valid()
}

#[inline(always)]
pub(crate) fn invalidate_queue<P: Placement>(registers: &mut WifiMacTxQueueControl<P>, queue: u8) {
    registers.modify_control_n(bank(queue), |writer| {
        writer.set_valid(false);
    });
}

#[inline(always)]
pub(crate) fn disable_queue<P: Placement>(registers: &mut WifiMacTxQueueControl<P>, queue: u8) {
    registers.modify_control_n(bank(queue), |writer| {
        writer.set_valid(false).set_enable(false);
    });
}

#[inline(always)]
pub(crate) fn publish_queue<P: Placement>(registers: &mut WifiMacTxQueueControl<P>, queue: u8) {
    registers.modify_control_n(bank(queue), |writer| {
        writer.set_valid(true).set_enable(true);
    });
}

/// Publish the three independent EDCA fields for one ordinary logical queue.
///
/// SOURCE: complete `libpp.a[hal_mac_tx.o]::hal_mac_tx_config_edca`.
/// Each field is a separate fresh-read RMW edge in this exact order.
#[inline(always)]
pub(crate) fn configure_edca<P: Placement>(
    registers: &mut WifiMacTxQueueControl<P>,
    queue: u8,
    aifsn: u8,
    contention_window: u16,
    interface: MacInterface,
) {
    let bank = bank(queue);
    registers.modify_config(bank, |writer| {
        writer.set_aifsn(bits(u32::from(aifsn)));
    });
    registers.modify_config(bank, |writer| {
        writer.set_contention_window(bits(u32::from(contention_window)));
    });
    registers.modify_config(bank, |writer| {
        writer.set_interface(interface.field());
    });
}

#[cfg(test)]
mod tests;
