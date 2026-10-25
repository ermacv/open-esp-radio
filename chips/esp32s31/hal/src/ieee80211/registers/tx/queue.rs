//! Ordinary EDCA TX queue register edges, each over one queue's physical
//! bank (`super::ordinary_bank`).

use oer_esp32s31_pac::{
    MacInterface, wifi_mac_tx_common::WifiMacTxCommon,
    wifi_mac_tx_queue_control::WifiMacTxQueueControl,
    wifi_mac_tx_queue_vector::WifiMacTxQueueVector,
};
use oer_mmio::{Bits, Index, Placement};

use super::MacTxProtection;
use crate::ieee80211::registers::every_index;

/// The four physical queue banks, in ascending order.
const BANKS: [Index<4>; 4] = every_index();

/// Replace both software request flags while the prepared queue is idle.
///
/// The descriptor-bound preparation owner calls this after vector formatting,
/// before ENABLE|VALID. A single fresh-read update preserves spacing and
/// replaces any CTS/RTS request retained from a previous PHY or attempt.
pub(crate) fn configure_protection<P: Placement>(
    control: &mut WifiMacTxQueueControl<P>,
    bank: Index<4>,
    protection: MacTxProtection,
) {
    control.modify_protection(bank, |w| {
        w.set_software_cts(matches!(protection, MacTxProtection::CtsToSelf))
            .set_software_rts(matches!(protection, MacTxProtection::RtsCts));
    });
}

/// Program the recovered RTS register edge without publishing a queue.
///
/// Complete `hal_he_set_tx_protection`: the software request preserves CTS
/// and spacing. A supplied HE threshold replaces the threshold word after
/// that RMW (the vendor's whole-word store: every field, the HE threshold
/// disable clear); `None` leaves it untouched, including its disabled state.
/// The threshold is in bytes, converted from the HE duration threshold for
/// the selected PHY by the vendor's `ic_get_he_rts_threshold_bytes`. It is
/// not an RTS/CTS Duration/NAV value. The caller must own that conversion and
/// the protected exchange before using this edge in an on-air transaction.
pub(crate) fn configure_rts<C: Placement, V: Placement>(
    control: &mut WifiMacTxQueueControl<C>,
    vector: &mut WifiMacTxQueueVector<V>,
    bank: Index<4>,
    enabled: bool,
    threshold_bytes: Option<u16>,
) {
    control.modify_protection(bank, |w| {
        w.set_software_rts(enabled);
    });
    if let Some(threshold_bytes) = threshold_bytes {
        vector.modify_he_rts_control(bank, |w| {
            w.set_rts_threshold_bytes(threshold_bytes)
                .set_rts_threshold_valid(true)
                .set_he_rts_disabled(false);
        });
    }
}

/// Disable the HE threshold in logical queue order, retaining byte-threshold state.
///
/// Complete `hal_he_disable_rts_threshold`; this does not clear software RTS.
/// Logical queues zero to three are physical banks three to zero.
pub(crate) fn disable_he_rts_threshold<P: Placement>(vector: &mut WifiMacTxQueueVector<P>) {
    let [b0, b1, b2, b3] = BANKS;
    for bank in [b3, b2, b1, b0] {
        vector.modify_he_rts_control(bank, |w| {
            w.set_he_rts_disabled(true);
        });
    }
}

pub(crate) fn set_cca_force<P: Placement>(registers: &mut WifiMacTxCommon<P>, value: Bits<2>) {
    registers.modify_cca_control(|w| {
        w.set_force(value);
    });
}

pub(crate) fn completion_pending<P: Placement>(registers: &WifiMacTxCommon<P>, queue: u8) -> bool {
    let state = registers.read_complete_state();
    match queue {
        0 => state.queue_0(),
        1 => state.queue_1(),
        2 => state.queue_2(),
        3 => state.queue_3(),
        _ => false,
    }
}

pub(crate) fn collision_pending<P: Placement>(registers: &WifiMacTxCommon<P>, queue: u8) -> bool {
    let state = registers.read_queue_state();
    match queue {
        0 => state.collision_queue_0(),
        1 => state.collision_queue_1(),
        2 => state.collision_queue_2(),
        3 => state.collision_queue_3(),
        _ => false,
    }
}

pub(crate) fn timeout_pending<P: Placement>(registers: &WifiMacTxCommon<P>, queue: u8) -> bool {
    let state = registers.read_queue_state();
    match queue {
        0 => state.timeout_queue_0(),
        1 => state.timeout_queue_1(),
        2 => state.timeout_queue_2(),
        3 => state.timeout_queue_3(),
        _ => false,
    }
}

pub(crate) fn queue_in_trigger_flow<P: Placement>(registers: &WifiMacTxCommon<P>, queue: u8) -> bool {
    let state = registers.read_queue_state();
    match queue {
        0 => state.trigger_flow_queue_0(),
        1 => state.trigger_flow_queue_1(),
        2 => state.trigger_flow_queue_2(),
        3 => state.trigger_flow_queue_3(),
        _ => false,
    }
}

/// The trigger-flow bits of every ordinary queue and the opaque upper
/// nibble, in the vendor's packed order, for a probe image.
#[cfg(oer_probe)]
pub(crate) fn trigger_flow_state_for_probe<P: Placement>(registers: &WifiMacTxCommon<P>) -> u32 {
    let state = registers.read_queue_state();
    u32::from(state.trigger_flow_queue_0())
        + u32::from(state.trigger_flow_queue_1()) * 2
        + u32::from(state.trigger_flow_queue_2()) * 4
        + u32::from(state.trigger_flow_queue_3()) * 8
        + state.bits_28_31_opaque().get() * 16
}

/// Acknowledge one queue's completion: the image the clear register reads
/// back, with the completed queue's bit.
///
/// SOURCE: BLOB_LIBPP_HAL_MAC_TX, PROMOTED_LMAC_TX.
pub(crate) fn acknowledge_completion<P: Placement>(registers: &mut WifiMacTxCommon<P>, queue: u8) {
    use oer_esp32s31_pac::wifi_mac_tx_common::MacTxCompletionConsumed;
    let consumed = match queue {
        0 => MacTxCompletionConsumed::Queue0,
        1 => MacTxCompletionConsumed::Queue1,
        2 => MacTxCompletionConsumed::Queue2,
        3 => MacTxCompletionConsumed::Queue3,
        _ => return,
    };
    let sample = registers.sample_mac_tx_completion();
    registers.acknowledge_mac_tx_completion(sample, consumed);
}

pub(crate) fn acknowledge_collision<P: Placement>(registers: &mut WifiMacTxCommon<P>, queue: u8) {
    match queue {
        0 => registers.trigger_queue_state_clear_collision_queue_0(),
        1 => registers.trigger_queue_state_clear_collision_queue_1(),
        2 => registers.trigger_queue_state_clear_collision_queue_2(),
        3 => registers.trigger_queue_state_clear_collision_queue_3(),
        _ => {}
    }
}

pub(crate) fn acknowledge_timeout<P: Placement>(registers: &mut WifiMacTxCommon<P>, queue: u8) {
    match queue {
        0 => registers.trigger_queue_state_clear_timeout_queue_0(),
        1 => registers.trigger_queue_state_clear_timeout_queue_1(),
        2 => registers.trigger_queue_state_clear_timeout_queue_2(),
        3 => registers.trigger_queue_state_clear_timeout_queue_3(),
        _ => {}
    }
}

pub(crate) fn queue_enabled<P: Placement>(registers: &WifiMacTxQueueControl<P>, bank: Index<4>) -> bool {
    registers.read_control(bank).enable()
}

pub(crate) fn queue_valid<P: Placement>(registers: &WifiMacTxQueueControl<P>, bank: Index<4>) -> bool {
    registers.read_control(bank).valid()
}

pub(crate) fn invalidate_queue<P: Placement>(registers: &mut WifiMacTxQueueControl<P>, bank: Index<4>) {
    registers.modify_control(bank, |w| {
        w.set_valid(false);
    });
}

pub(crate) fn disable_queue<P: Placement>(registers: &mut WifiMacTxQueueControl<P>, bank: Index<4>) {
    registers.modify_control(bank, |w| {
        w.set_valid(false).set_enable(false);
    });
}

pub(crate) fn publish_queue<P: Placement>(registers: &mut WifiMacTxQueueControl<P>, bank: Index<4>) {
    registers.modify_control(bank, |w| {
        w.set_valid(true).set_enable(true);
    });
}

/// Publish the three independent EDCA fields for one ordinary logical queue.
///
/// SOURCE: complete `libpp.a[hal_mac_tx.o]::hal_mac_tx_config_edca`.
/// Each field is a separate fresh-read RMW edge in this exact order.
pub(crate) fn configure_edca<P: Placement>(
    registers: &mut WifiMacTxQueueControl<P>,
    bank: Index<4>,
    aifsn: Bits<4>,
    contention_window: Bits<10>,
    interface: MacInterface,
) {
    registers.modify_config(bank, |w| {
        w.set_aifsn(aifsn);
    });
    registers.modify_config(bank, |w| {
        w.set_contention_window(contention_window);
    });
    registers.modify_config(bank, |w| {
        w.set_interface(Bits::truncate(interface.bits()));
    });
}

#[cfg(test)]
mod tests;
