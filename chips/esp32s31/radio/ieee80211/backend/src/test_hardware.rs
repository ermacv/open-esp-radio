//! The host test double of the MAC's transmit queues and key table, and
//! the helpers of the tests of the ordinary transmitter, the management
//! retries and the key erasure.

use core::{cell::RefCell, pin::Pin};
use std::{boxed::Box, vec::Vec};

use crate::mac::crypto::CcmpKeyHardware;
use crate::mac::tx::{CommitTxDma, PreparedTxDma, TxHardware, TxSlot};
use crate::ordinary_tx::{WifiTxPowerPair, WifiTxPowerProfile};
use crate::tx::WifiTxWake;
use oer_chips_esp32s31_hal::layouts::{DmaRef, WifiTxDmaDescriptor};
use oer_chips_esp32s31_hal::types::{
    MacCcmpKeyIdentity, MacHeTxProgram, MacHtAmpduCompletionObservation, MacHtTxProgram,
    MacKeyInstallOutcome, MacLegacyTxProgram, MacTxAbortCause, MacTxAbortOutcome,
    MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason, MacTxPrepareRefused,
    MacTxQueueAborted, MacTxQueueDetached,
};
use oer_protocols_ieee80211_contracts_port::PhyRate;
use oer_protocols_ieee80211_wire::phy::LegacyRate;

pub(crate) const STATION: [u8; 6] = [0x02, 0x03, 0x04, 0x05, 0x06, 0x07];

pub(crate) const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];

pub(crate) const PEER: [u8; 6] = [0x30, 0x31, 0x32, 0x33, 0x34, 0x35];

pub(crate) const TIMEOUT: u64 = 250_000;

/// The hardware index of the best-effort queue, where the helpers' attempts
/// go.
pub(crate) const BE: usize = 2;

/// Register owner model with the four ordinary queues' latched state.
#[derive(Debug, PartialEq)]
pub(crate) enum CryptoOperation {
    Association(
        oer_chips_esp32s31_hal::types::MacInterface,
        oer_chips_esp32s31_hal::types::MacSppMode,
    ),
    Install(u8),
    Remove(u8),
}

#[derive(Default)]
pub(crate) struct Hardware {
    pub(crate) legacy: Vec<(u8, MacLegacyTxProgram)>,
    /// Each legacy publication's program, in order; a fixture may keep a
    /// clone to read the live publication order.
    pub(crate) published: std::rc::Rc<RefCell<Vec<MacLegacyTxProgram>>>,
    pub(crate) ht: Vec<(u8, MacHtTxProgram)>,
    pub(crate) he: Vec<(u8, MacHeTxProgram)>,
    pub(crate) completion: [Option<MacTxCompletionObservation>; 4],
    pub(crate) block_ack_completion: [Option<MacHtAmpduCompletionObservation>; 4],
    pub(crate) timeout_pending: [bool; 4],
    /// A queue whose abort finds VALID clear: the hardware took the PPDU.
    pub(crate) taken_by_air: [bool; 4],
    pub(crate) collision_pending: [bool; 4],
    /// The queue aborts taken, each one step.
    pub(crate) aborts: usize,
    pub(crate) installed_keys: Vec<(u8, MacCcmpKeyIdentity)>,
    pub(crate) key_material: Vec<(u8, [u8; 16])>,
    pub(crate) reject_key_writes: usize,
    /// The key table reads every entry valid however it was cleared.
    pub(crate) key_entries_stuck: bool,
    pub(crate) crypto_order: Vec<CryptoOperation>,
    pub(crate) cleared_keys: Vec<u8>,
}

impl Hardware {
    pub(crate) fn publications(&self) -> usize {
        self.legacy.len() + self.ht.len() + self.he.len()
    }
}

impl TxHardware for Hardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        program: MacLegacyTxProgram,
    ) -> Result<(), MacTxPrepareRefused> {
        let queue = queue.get() as u8;
        self.legacy.push((queue, program));
        self.published.borrow_mut().push(program);
        Ok(())
    }

    fn start_bound_tx<D: CommitTxDma>(
        &mut self,
        publication: D,
        _queue: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        handed_over: impl FnOnce(),
    ) -> Result<(), MacTxPrepareRefused> {
        handed_over();
        publication.commit(|_started| {});
        Ok(())
    }

    fn prepare_bound_ht_tx(
        &mut self,
        _dma: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        program: MacHtTxProgram,
    ) -> Result<(), MacTxPrepareRefused> {
        let queue = queue.get() as u8;
        self.ht.push((queue, program));
        Ok(())
    }

    fn prepare_bound_he_tx(
        &mut self,
        _dma: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        program: MacHeTxProgram,
    ) -> Result<(), MacTxPrepareRefused> {
        let queue = queue.get() as u8;
        self.he.push((queue, program));
        Ok(())
    }

    fn take_tx_completion(
        &mut self,
        queue: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
    ) -> Option<MacTxCompletionObservation> {
        let queue = queue.get() as u8;
        self.completion[usize::from(queue)].take()
    }

    fn take_block_ack_completion(
        &mut self,
        queue: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
    ) -> Option<MacHtAmpduCompletionObservation> {
        let queue = queue.get() as u8;
        self.block_ack_completion[usize::from(queue)].take()
    }

    fn abort_tx_queue<C: oer_portable_time::Clock + ?Sized, R>(
        &mut self,
        queue: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        cause: MacTxAbortCause,
        _clock: &C,
        aborted: impl for<'aborted> FnOnce(MacTxQueueAborted<'aborted>) -> R,
    ) -> MacTxAbortOutcome<R> {
        let queue = usize::from(queue.get() as u8);
        if cause == MacTxAbortCause::TimeoutEdge && !self.timeout_pending[queue] {
            return MacTxAbortOutcome::NoEdge;
        }
        self.aborts += 1;
        self.timeout_pending[queue] = false;
        if self.taken_by_air[queue] {
            return MacTxAbortOutcome::OnAir;
        }
        MacTxAbortOutcome::Aborted(aborted(MacTxQueueAborted::new_model()))
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        queue: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        let queue = queue.get() as u8;
        let queue = usize::from(queue);
        let pending = match reason {
            MacTxDetachReason::Collision => core::mem::take(&mut self.collision_pending[queue]),
            MacTxDetachReason::Completed => true,
        };
        if pending {
            MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                expected_descriptor_head,
            )))
        } else {
            MacTxDetachOutcome::NoEvent
        }
    }
}

impl CcmpKeyHardware for Hardware {
    fn configure_ccmp_association(
        &mut self,
        interface: oer_chips_esp32s31_hal::types::MacInterface,
        mode: oer_chips_esp32s31_hal::types::MacSppMode,
    ) {
        self.crypto_order
            .push(CryptoOperation::Association(interface, mode));
    }
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        if self.reject_key_writes != 0 {
            self.reject_key_writes -= 1;
            return MacKeyInstallOutcome::Rejected;
        }
        if self.installed_keys.iter().any(|(slot, _)| *slot == index) {
            return MacKeyInstallOutcome::Occupied;
        }
        self.crypto_order.push(CryptoOperation::Install(index));
        self.installed_keys.push((index, identity));
        self.key_material.push((index, *temporal_key));
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        self.installed_keys.retain(|(slot, _)| *slot != index);
        self.key_material.retain(|(slot, _)| *slot != index);
        self.crypto_order.push(CryptoOperation::Remove(index));
        self.cleared_keys.push(index);
    }

    fn ccmp_entry_is_valid(
        &mut self,
        index: oer_chips_esp32s31_hal::types::MacKeyEntryIndex,
    ) -> bool {
        self.key_entries_stuck
            || self
                .installed_keys
                .iter()
                .any(|(slot, _)| u32::from(*slot) == index.get())
    }
}

pub(crate) struct Power;

impl WifiTxPowerProfile for Power {
    fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
        WifiTxPowerPair {
            primary: 5,
            alternate: 6,
        }
    }
}

pub(crate) type Timer = oer_portable_time_virtual::SkipClock;

pub(crate) fn entropy() -> u32 {
    0x1234_5678
}

/// A model slot in permanently retained storage, as target SRAM is.
pub(crate) fn slot() -> Pin<&'static mut TxSlot<512>> {
    Pin::static_mut(Box::leak(Box::new(TxSlot::new_model().unwrap())))
}

/// A QoS Data MPDU from the station to the BSS.
pub(crate) fn data_frame(receiver: [u8; 6]) -> [u8; 26] {
    let mut frame = [0; 26];
    frame[0] = 0x88;
    frame[1] = 0x01;
    frame[4..10].copy_from_slice(&receiver);
    frame[10..16].copy_from_slice(&STATION);
    frame[16..22].copy_from_slice(&BSSID);
    frame
}

/// The rate the attempts go at by default, 24 Mb/s OFDM.
pub(crate) const OFDM24: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm24M);

pub(crate) fn interrupt(events: u32) -> WifiTxWake {
    WifiTxWake::Interrupt { events }
}

pub(crate) fn queued_free_slot<const N: usize>(
    slot: Pin<&mut TxSlot<N>>,
) -> crate::mac::tx::TxSlotFree<'_, N> {
    let crate::mac::tx::TxSlotAccess::Free(free) = slot.access() else {
        panic!("the fixture's queued slot is free")
    };
    free
}
