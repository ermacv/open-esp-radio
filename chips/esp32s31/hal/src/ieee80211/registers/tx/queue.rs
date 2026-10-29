//! The register leaves of ordinary EDCA TX queue ownership.
//!
//! Logical queue `q` lives in physical bank `3 - q` of the queue-control and
//! queue-vector arrays; every leaf here takes the logical queue.

use crate::types::MacTxQueueIndex;
use oer_esp32s31_pac::wifi_mac::{
    AcknowledgeMacTxCompletionConsumed, WifiMacTxCommon, WifiMacTxQueueControl,
    WifiMacTxQueueVector,
};
use oer_mmio::{Index, Placement};

use super::MacTxProtection;
use crate::ieee80211::registers::{MacInterface, bits};

/// The register-array element of an ordinary logical queue.
#[inline(always)]
pub(crate) const fn bank(queue: MacTxQueueIndex) -> Index<4> {
    match queue {
        MacTxQueueIndex::Zero => Index::constant::<3>(),
        MacTxQueueIndex::One => Index::constant::<2>(),
        MacTxQueueIndex::Two => Index::constant::<1>(),
        MacTxQueueIndex::Three => Index::constant::<0>(),
    }
}

/// The upper half of the eight-entry HE array holds ordinary queues.
#[inline(always)]
pub(crate) const fn he_queue_control(queue: MacTxQueueIndex) -> Index<8> {
    match queue {
        MacTxQueueIndex::Zero => Index::constant::<7>(),
        MacTxQueueIndex::One => Index::constant::<6>(),
        MacTxQueueIndex::Two => Index::constant::<5>(),
        MacTxQueueIndex::Three => Index::constant::<4>(),
    }
}

/// Replace both software request flags while the prepared queue is idle.
///
/// The descriptor-bound preparation owner calls this after vector formatting,
/// before ENABLE|VALID. A single fresh-read update preserves spacing and
/// replaces any CTS/RTS request retained from a previous PHY or attempt.
#[inline(always)]
pub(crate) fn configure_protection<P: Placement>(
    control: &mut WifiMacTxQueueControl<P>,
    queue: MacTxQueueIndex,
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
    queue: MacTxQueueIndex,
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
    queue: MacTxQueueIndex,
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
    for queue in MacTxQueueIndex::ALL {
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
pub(crate) fn completion_pending<P: Placement>(
    registers: &WifiMacTxCommon<P>,
    queue: MacTxQueueIndex,
) -> bool {
    let state = registers.read_complete_state();
    match queue {
        MacTxQueueIndex::Zero => state.queue_0(),
        MacTxQueueIndex::One => state.queue_1(),
        MacTxQueueIndex::Two => state.queue_2(),
        MacTxQueueIndex::Three => state.queue_3(),
    }
}

#[inline(always)]
pub(crate) fn collision_pending<P: Placement>(
    registers: &WifiMacTxCommon<P>,
    queue: MacTxQueueIndex,
) -> bool {
    let state = registers.read_queue_state();
    match queue {
        MacTxQueueIndex::Zero => state.collision_queue_0(),
        MacTxQueueIndex::One => state.collision_queue_1(),
        MacTxQueueIndex::Two => state.collision_queue_2(),
        MacTxQueueIndex::Three => state.collision_queue_3(),
    }
}

#[inline(always)]
pub(crate) fn timeout_pending<P: Placement>(
    registers: &WifiMacTxCommon<P>,
    queue: MacTxQueueIndex,
) -> bool {
    let state = registers.read_queue_state();
    match queue {
        MacTxQueueIndex::Zero => state.timeout_queue_0(),
        MacTxQueueIndex::One => state.timeout_queue_1(),
        MacTxQueueIndex::Two => state.timeout_queue_2(),
        MacTxQueueIndex::Three => state.timeout_queue_3(),
    }
}

#[inline(always)]
pub(crate) fn queue_in_trigger_flow<P: Placement>(
    registers: &WifiMacTxCommon<P>,
    queue: MacTxQueueIndex,
) -> bool {
    let state = registers.read_queue_state();
    match queue {
        MacTxQueueIndex::Zero => state.trigger_flow_queue_0(),
        MacTxQueueIndex::One => state.trigger_flow_queue_1(),
        MacTxQueueIndex::Two => state.trigger_flow_queue_2(),
        MacTxQueueIndex::Three => state.trigger_flow_queue_3(),
    }
}

/// The completion event of ordinary logical queue `queue`.
const fn completion_event(queue: MacTxQueueIndex) -> AcknowledgeMacTxCompletionConsumed {
    match queue {
        MacTxQueueIndex::Zero => AcknowledgeMacTxCompletionConsumed::Queue0,
        MacTxQueueIndex::One => AcknowledgeMacTxCompletionConsumed::Queue1,
        MacTxQueueIndex::Two => AcknowledgeMacTxCompletionConsumed::Queue2,
        MacTxQueueIndex::Three => AcknowledgeMacTxCompletionConsumed::Queue3,
    }
}

/// Acknowledge the completion of `queue` with every event the clear word
/// holds now.
///
/// SOURCE: BLOB_LIBPP_HAL_MAC_TX, PROMOTED_LMAC_TX: the vendor reads
/// `COMPLETE_CLEAR` and writes it back with the completed queue's bit set.
#[inline(always)]
pub(crate) fn acknowledge_completion<P: Placement>(
    registers: &mut WifiMacTxCommon<P>,
    queue: MacTxQueueIndex,
) {
    let consumed = completion_event(queue);
    let sampled = registers.sample_acknowledge_mac_tx_completion();
    registers.acknowledge_acknowledge_mac_tx_completion(sampled, Some(consumed));
}

#[inline(always)]
pub(crate) fn acknowledge_collision<P: Placement>(
    registers: &mut WifiMacTxCommon<P>,
    queue: MacTxQueueIndex,
) {
    match queue {
        MacTxQueueIndex::Zero => registers.trigger_queue_state_clear_collision_queue_0(),
        MacTxQueueIndex::One => registers.trigger_queue_state_clear_collision_queue_1(),
        MacTxQueueIndex::Two => registers.trigger_queue_state_clear_collision_queue_2(),
        MacTxQueueIndex::Three => registers.trigger_queue_state_clear_collision_queue_3(),
    }
}

#[inline(always)]
pub(crate) fn acknowledge_timeout<P: Placement>(
    registers: &mut WifiMacTxCommon<P>,
    queue: MacTxQueueIndex,
) {
    match queue {
        MacTxQueueIndex::Zero => registers.trigger_queue_state_clear_timeout_queue_0(),
        MacTxQueueIndex::One => registers.trigger_queue_state_clear_timeout_queue_1(),
        MacTxQueueIndex::Two => registers.trigger_queue_state_clear_timeout_queue_2(),
        MacTxQueueIndex::Three => registers.trigger_queue_state_clear_timeout_queue_3(),
    }
}

#[inline(always)]
pub(crate) fn queue_enabled<P: Placement>(
    registers: &WifiMacTxQueueControl<P>,
    queue: MacTxQueueIndex,
) -> bool {
    registers.read_control_n(bank(queue)).enable()
}

#[inline(always)]
pub(crate) fn queue_valid<P: Placement>(
    registers: &WifiMacTxQueueControl<P>,
    queue: MacTxQueueIndex,
) -> bool {
    registers.read_control_n(bank(queue)).valid()
}

#[inline(always)]
pub(crate) fn invalidate_queue<P: Placement>(
    registers: &mut WifiMacTxQueueControl<P>,
    queue: MacTxQueueIndex,
) {
    registers.modify_control_n(bank(queue), |writer| {
        writer.set_valid(false);
    });
}

#[inline(always)]
pub(crate) fn disable_queue<P: Placement>(
    registers: &mut WifiMacTxQueueControl<P>,
    queue: MacTxQueueIndex,
) {
    registers.modify_control_n(bank(queue), |writer| {
        writer.set_valid(false).set_enable(false);
    });
}

#[inline(always)]
pub(crate) fn publish_queue<P: Placement>(
    registers: &mut WifiMacTxQueueControl<P>,
    queue: MacTxQueueIndex,
) {
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
    queue: MacTxQueueIndex,
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
        writer.set_interface(oer_esp32s31_pac::wifi_mac::config::Interface::from_field(
            interface.bits(),
        ));
    });
}

#[cfg(test)]
mod tests;
