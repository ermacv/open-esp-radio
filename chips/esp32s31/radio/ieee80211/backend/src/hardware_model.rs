//! Host model of the hardware atoms under the real backend (test support).
//!
//! The port, the device, the receive session and the HAL's DMA owners run
//! unchanged above it: [`ModelRegisters`] stands where the chip has
//! `RadioAccess`, [`ModelFrontier`] where it has `WifiStopped`, the HAL's
//! `RxWalkerModel` (through its `SharedRxWalker`) under the walker and
//! [`MacInterruptModel`] under the line. A test plays the air: it reads the
//! PPDUs ([`Atoms::take_air`]), ends them ([`Atoms::end_ppdu`]) and lands
//! received units ([`ModelRx::land`]).
//!
//! # Rules
//!
//! Every behaviour here is one rule with its evidence, named at its code; a
//! behaviour without evidence is a defect, and what has none is an input of
//! the test instead, never a claim about the chip.
//!
//! | Rule | Behaviour | Evidence |
//! | --- | --- | --- |
//! | `RX-LAND` | the walker lands a unit where NEXT points | `HIL_W1_RX_WALKER_2026_10_18`, `HIL_W1_OWNER_BIT_2026_10_18`, in `RxWalkerModel` |
//! | `RX-PREFIX` | a unit is the RX-control prefix, its tail, the MPDU | layouts `WIFI_RX_CONTROL_PREFIX`, `WIFI_RX_CONTROL_TAIL`; `HW24_RX_POST_IRQ_BYTES_20261027` |
//! | `RX-EDGE` | a landed unit latches RX success, masked or not, its bytes already readable | `HW24_RX_MASKED_W1C_20261027`, `HW24_RX_POST_IRQ_BYTES_20261027` |
//! | `IRQ-LINE` | the line stays asserted while an enabled event is latched | `BLOB_LIBPP_WDEV_PROCESS_FIQ`, in [`MacInterruptModel`] |
//! | `TX-CHAIN` | a started queue follows its chain to the last descriptor | layout `WIFI_TX_DMA_DESCRIPTOR`; `BLOB_LIBPP_ORDINARY_TX_OWNERSHIP_EDGES` |
//! | `TX-PSDU` | a buffer is the metadata prefix, then the PSDU its low twenty bits count | `BLOB_LIBPP_PP_HE_SMPDU_LENGTH`; layout `WIFI_TX_METADATA_HEAD` |
//! | `TX-DONE` | an ended PPDU latches its queue's completion and TX complete | `BLOB_LIBPP_ORDINARY_TX_OWNERSHIP_EDGES`, `BLOB_LIBPP_WDEV_PROCESS_FIQ` |
//! | `TX-DETACH` | a taken completion's disable ends hardware ownership | `BLOB_LIBPP_ORDINARY_TX_OWNERSHIP_EDGES` |
//! | `STOP-RUN` | a stop request leaves the MAC active, then idle | `BLOB_LIBPP_MAC_CHANNEL_SWITCH` |
//!
//! # Adverse inputs
//!
//! Abort under forced CCA and how it ends, the collision bit, the tail's
//! `rx_state`, the key at capture: no stand evidence. A test may inject them,
//! which today means an abort's end ([`Atoms::abort_finds_on_air`]) and the
//! prefix's fields ([`RxPrefix`]); a case that passes with them proves
//! nothing about the chip. When a PPDU ends, its status and how long the MAC
//! stays active are the test's inputs too.
//!
//! A call outside these rules goes in [`Atoms::rule_breaks`], for the test to
//! fail on: the model answers the product without panicking.

use core::{cell::RefCell, convert::Infallible, task::Poll};
use std::{boxed::Box, rc::Rc, vec, vec::Vec};

use oer_chips_esp32s31_hal::{
    layouts::{DmaRef, WifiTxDmaDescriptor},
    radio::ieee80211::dma::{
        rx_dma::model::{RxWalkerLanding, RxWalkerModel, SharedRxWalker},
        rx_ring::{RxRingCoreReady, RxRingPrepared},
        rx_storage::{HostDmaAccessError, RxDmaStorage},
        space::HostDma,
        tx_storage::{CommitTxDma, TxChainUnreadable},
    },
    system::owner::maintenance,
    types::{
        MacCcmpKeyIdentity, MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
        MacHeTriggerTxQueueSnapshot, MacHeTxProgram, MacHtAmpduCompletionObservation,
        MacHtTxProgram, MacInterface, MacInterruptEvents, MacInterruptObservation,
        MacKeyInstallOutcome, MacLegacyTxProgram, MacPti, MacSppMode, MacStaApReceivePlan,
        MacTxAbortCause, MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason,
        MacTxPrepareFault, MacTxPrepareRefused, MacTxQueueDetached, MacTxQueueIndex,
        StaTbttSchedule,
    },
};
use oer_chips_family_platform_interrupt_table::{
    Context as HandlerContext, signal::SignalExecutor,
};
use oer_portable_memory::{HandlerCell, PreparedTxDma};
use oer_protocols_ieee80211_contracts_port::RegulatedChannel;
use oer_protocols_ieee80211_wire::{channel::WifiChannel, sequence::SequenceNumber};
use oer_protocols_radio_regulatory::{Permitted, TxCeiling};

use crate::{
    cooperative_hardware::ReceiveRegisters,
    irq::{DoorbellAcknowledge, RadioDoorbell, WifiIrqLine, WifiIrqPart, WifiIrqRequests},
    lower_mac::{RxBeaconPriorityHardware, StationTbttHardware, TxGateHardware},
    mac::{
        ap_policy::ApRxPolicyHardware,
        ap_tsf::{AccessPointTsfWrite, ApTsfHardware},
        crypto::CcmpKeyHardware,
        init::{
            MacRuntimeStartHardware, MacRuntimeStopHardware, MacSnifferHardware,
            StaEspNowRxPolicyHardware, StaLinkRxPolicyHardware, StaNoiseFloorHardware,
            StaScanRxPolicyHardware,
        },
        irq::model::{MacInterruptModel, PowerInterruptModel},
        rx::{
            PUBLIC_HEADER_SIZE, RxBufferFullCounter, RxRingLive,
            hardware::{RxBlockAckHardware, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
        },
        sta_ap_registers::StaApRegisterHardware,
        tx::{TxHardware, ampdu::HtAmpduHardware},
    },
    ordinary_tx::{WifiTxPowerPair, WifiTxPowerProfile},
    port::{WifiRetuneError, device::Frontier, device::RetuneEnd},
    station_tsf::{StationTsfHardware, StationTsfWrite},
};

/// The MPDU's offset in a transmit buffer (`TX-PSDU`).
const TX_PREFIX: usize = crate::mac::tx::ampdu::TX_AMPDU_METADATA_SIZE;
/// The FCS the hardware appends (`TX-PSDU`).
const FCS: usize = 4;
/// The metadata word's PSDU length (`TX-PSDU`).
const METADATA_LENGTH: u32 = (1 << 20) - 1;
/// Entries one interrupt may take before the line counts as stuck.
const LINE_ENTRIES: usize = 16;

/// How a queue's PPDU was programmed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AirFormat {
    Legacy,
    Ht,
    He,
}

/// One PPDU a started queue read from its chain (`TX-CHAIN`, `TX-PSDU`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AirFrame {
    /// The hardware queue.
    pub queue: u8,
    pub format: AirFormat,
    /// Each descriptor's MPDU, without the FCS.
    pub mpdus: Vec<Vec<u8>>,
}

/// What the product did that the model's rules do not take, or a call
/// outside the path the model covers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleBreak {
    /// A chain the model could not follow (`TX-CHAIN`).
    Chain(TxChainUnreadable),
    /// A buffer whose metadata does not count its used bytes (`TX-PSDU`).
    Metadata { used: usize, counted: usize },
    /// A queue started without a prepared chain, or with another's.
    NotNamed,
    /// A queue prepared while its PPDU was on the air.
    QueueBusy,
    /// A detach of a queue whose PPDU has not ended (`TX-DETACH`).
    DetachOnAir { queue: u8 },
    /// A call the model does not cover.
    Unmodelled(&'static str),
    /// The line stayed asserted past its bound (`IRQ-LINE`).
    LineStuck,
    /// The interrupt handler found the bank's fatal cause.
    Fatal(u32),
}

/// The work the atoms saw: observed numbers, for a report.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AtomCounts {
    /// Register owner calls that read or wrote the MAC.
    pub register_calls: u32,
    /// PPDUs started.
    pub ppdus: u32,
    /// Bytes the DMA read from transmit chains.
    pub tx_dma_bytes: u32,
    /// Units landed in the ring.
    pub units_landed: u32,
    /// Bytes the DMA wrote to the ring.
    pub rx_dma_bytes: u32,
    /// Entries of the interrupt handler.
    pub interrupt_entries: u32,
    /// Doorbell rings.
    pub doorbell_rings: u32,
    /// Queue aborts begun under forced CCA.
    pub aborts: u32,
}

/// One queue's state as the MAC holds it.
#[derive(Default)]
struct Queue {
    /// The descriptor head the queue names since its preparation.
    named: Option<u32>,
    format: Option<AirFormat>,
    /// A PPDU is out: the chain belongs to the hardware.
    on_air: bool,
    completion: Option<MacTxCompletionObservation>,
    block_ack: Option<MacHtAmpduCompletionObservation>,
    /// The PPDU ended and no detach has taken the chain back.
    ended: bool,
    /// What the next timeout detach finds: the hardware took the PPDU.
    taken_by_air: bool,
}

struct State {
    queues: [Queue; 4],
    air: Vec<AirFrame>,
    keys: Vec<(u8, MacCcmpKeyIdentity)>,
    block_acks: Vec<u8>,
    /// The MAC runtime runs.
    running: bool,
    /// A stop request's active reads left (`STOP-RUN`).
    active_reads: Option<u32>,
    /// How many reads a stop request stays active: the test's input.
    stop_reads: u32,
    promiscuous: bool,
    station_tsf: u64,
    tbtt: Option<StaTbttSchedule>,
    tx_blocked: bool,
    /// The HE BSS the MAC matches (its color, enabled, partial) and the
    /// HE20 association it holds; `None` after a removal or before any.
    he_bss: Option<(oer_chips_esp32s31_hal::types::MacHeBssColor, bool, bool)>,
    he: Option<oer_chips_esp32s31_hal::types::MacHe20Association>,
    cca_forced: bool,
    breaks: Vec<RuleBreak>,
    counts: AtomCounts,
    clock: Rc<dyn Fn() -> u64>,
}

/// The atoms' state: the test's handle on what the device's registers touch.
#[derive(Clone)]
pub struct Atoms(Rc<RefCell<State>>);

impl Atoms {
    /// Atoms whose MAC clock reads `clock`'s microseconds.
    pub fn new(clock: Rc<dyn Fn() -> u64>) -> Self {
        Self(Rc::new(RefCell::new(State {
            queues: Default::default(),
            air: Vec::new(),
            keys: Vec::new(),
            block_acks: Vec::new(),
            running: false,
            active_reads: None,
            stop_reads: 1,
            promiscuous: false,
            station_tsf: 0,
            tbtt: None,
            tx_blocked: false,
            he_bss: None,
            he: None,
            cca_forced: false,
            breaks: Vec::new(),
            counts: AtomCounts::default(),
            clock,
        })))
    }

    /// The HE BSS the MAC matches now.
    pub fn he_bss(&self) -> Option<(oer_chips_esp32s31_hal::types::MacHeBssColor, bool, bool)> {
        self.0.borrow().he_bss
    }

    /// The HE20 association the MAC holds now.
    pub fn he(&self) -> Option<oer_chips_esp32s31_hal::types::MacHe20Association> {
        self.0.borrow().he
    }

    /// The PPDUs started since the last call.
    pub fn take_air(&self) -> Vec<AirFrame> {
        core::mem::take(&mut self.0.borrow_mut().air)
    }

    /// Whether `queue` holds a PPDU the air has not ended.
    pub fn on_air(&self, queue: u8) -> bool {
        self.queue(queue, |queue| queue.on_air && !queue.ended)
    }

    /// `TX-DONE`: the PPDU of `queue` ended with the input `status` and
    /// `detail`. The event for the line comes back to its caller.
    #[must_use]
    pub fn end_ppdu(&self, queue: u8, status: u8, detail: u8) -> MacInterruptObservation {
        self.queue(queue, |queue| {
            queue.completion = Some(MacTxCompletionObservation::new_model(status, detail));
            queue.ended = true;
        });
        MacInterruptObservation::from_semantic_events(MacInterruptEvents::TX_COMPLETE, false, false)
    }

    /// An adverse input: the next abort of `queue` finds the hardware took
    /// the PPDU, which stays out.
    pub fn abort_finds_on_air(&self, queue: u8) {
        self.queue(queue, |queue| queue.taken_by_air = true);
    }

    /// An input: a stop request stays active for this many reads.
    pub fn set_stop_reads(&self, reads: u32) {
        self.0.borrow_mut().stop_reads = reads;
    }

    /// Every rule the product broke, in order.
    pub fn rule_breaks(&self) -> Vec<RuleBreak> {
        self.0.borrow().breaks.clone()
    }

    /// The work the atoms saw.
    pub fn counts(&self) -> AtomCounts {
        self.0.borrow().counts
    }

    /// Whether the MAC runtime runs.
    pub fn running(&self) -> bool {
        self.0.borrow().running
    }

    /// Whether the open receive policy is on.
    pub fn promiscuous(&self) -> bool {
        self.0.borrow().promiscuous
    }

    /// The key-table entries that hold a key, in installation order.
    pub fn installed_keys(&self) -> Vec<u8> {
        self.0.borrow().keys.iter().map(|(slot, _)| *slot).collect()
    }

    /// Put a key in the key table's entry `index`, as an earlier
    /// installation's write left it.
    pub fn seed_key(&self, index: u8, identity: MacCcmpKeyIdentity) {
        self.0.borrow_mut().keys.push((index, identity));
    }

    fn queue<R>(&self, queue: u8, f: impl FnOnce(&mut Queue) -> R) -> R {
        let mut state = self.0.borrow_mut();
        let mut vacant = Queue::default();
        let place = state
            .queues
            .get_mut(usize::from(queue))
            .unwrap_or(&mut vacant);
        f(place)
    }

    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut state = self.0.borrow_mut();
        state.counts.register_calls = state.counts.register_calls.saturating_add(1);
        f(&mut state)
    }

    fn count(&self, f: impl FnOnce(&mut AtomCounts)) {
        f(&mut self.0.borrow_mut().counts);
    }

    fn broke(&self, rule: RuleBreak) {
        self.0.borrow_mut().breaks.push(rule);
    }

    fn unmodelled(&self, call: &'static str) {
        self.broke(RuleBreak::Unmodelled(call));
    }
}

/// The register owner over the model: where the chip has `RadioAccess`.
pub struct ModelRegisters {
    atoms: Atoms,
    walker: SharedRxWalker<'static>,
}

impl ModelRegisters {
    pub fn new(atoms: Atoms, walker: &'static RefCell<RxWalkerModel>) -> Self {
        Self {
            atoms,
            walker: SharedRxWalker(walker),
        }
    }

    fn prepare(
        &mut self,
        head: DmaRef<WifiTxDmaDescriptor>,
        queue: MacTxQueueIndex,
        format: AirFormat,
    ) -> Result<(), MacTxPrepareRefused> {
        let index = queue.get() as u8;
        let busy = self.atoms.queue(index, |place| {
            if place.on_air {
                return true;
            }
            place.named = Some(head.bits());
            place.format = Some(format);
            false
        });
        if busy {
            self.atoms.broke(RuleBreak::QueueBusy);
            return Err(MacTxPrepareRefused::QueueActive);
        }
        Ok(())
    }
}

/// The MAC, as one transaction borrows it.
pub struct ModelMac<'a>(&'a Atoms);

impl ReceiveRegisters for ModelRegisters {
    type Mac<'a> = ModelMac<'a>;
    type RxDma = SharedRxWalker<'static>;
    fn mac(&mut self) -> ModelMac<'_> {
        ModelMac(&self.atoms)
    }
    fn split_rx_dma(&mut self) -> (ModelMac<'_>, &mut SharedRxWalker<'static>) {
        (ModelMac(&self.atoms), &mut self.walker)
    }
    fn mac_local_time(&mut self) -> u32 {
        self.atoms.with(|state| (state.clock)() as u32)
    }
    #[cfg(oer_trace)]
    fn sample_mac_counts(&mut self, _now_us: u32) {}
    /// The model has no statistics bank.
    #[cfg(oer_trace)]
    fn mac_banks(
        &mut self,
    ) -> (
        Option<crate::rx::counters::mac::MacBank>,
        Option<crate::rx::counters::mac::MacSample>,
    ) {
        (None, None)
    }
}

impl MacRuntimeStartHardware for ModelMac<'_> {
    fn prepare_mac_runtime<'a, const COUNT: usize>(
        &mut self,
        ring: RxRingPrepared<'a, COUNT>,
    ) -> RxRingCoreReady<'a, COUNT> {
        ring.core_ready_for_test()
    }
    fn start_mac_runtime<const COUNT: usize>(&mut self, _ring: &RxRingLive<'_, COUNT>) {
        self.0.with(|state| {
            state.running = true;
            state.active_reads = None;
        });
    }
}

impl MacRuntimeStopHardware for ModelMac<'_> {
    /// `STOP-RUN`: the request leaves the MAC active.
    fn request_mac_runtime_stop(&mut self) {
        self.0.with(|state| {
            state.running = false;
            state.active_reads = Some(state.stop_reads);
        });
    }
    /// `STOP-RUN`: active, then idle.
    fn mac_runtime_active_state(&mut self) -> u8 {
        self.0.with(|state| match state.active_reads {
            Some(left) => {
                state.active_reads = left.checked_sub(1);
                u8::from(left != 0)
            }
            None => u8::from(state.running),
        })
    }
    fn resume_mac_runtime(&mut self) {
        self.0.with(|state| {
            state.running = true;
            state.active_reads = None;
        });
    }
}

impl MacSnifferHardware for ModelMac<'_> {
    fn configure_open_promiscuous_receive(&mut self) {
        self.0.with(|state| state.promiscuous = true);
    }
    fn disable_open_promiscuous_receive(&mut self) {
        self.0.with(|state| state.promiscuous = false);
    }
}

impl RxBufferFullCounter for ModelMac<'_> {
    fn buffer_full_count(&mut self) -> Option<u16> {
        None
    }
}

impl TxHardware for ModelRegisters {
    fn prepare_bound_legacy_tx(
        &mut self,
        dma: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: MacTxQueueIndex,
        _program: MacLegacyTxProgram,
    ) -> Result<(), MacTxPrepareRefused> {
        self.prepare(dma.descriptor_head(), queue, AirFormat::Legacy)
    }

    fn prepare_bound_ht_tx(
        &mut self,
        dma: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: MacTxQueueIndex,
        _program: MacHtTxProgram,
    ) -> Result<(), MacTxPrepareRefused> {
        self.prepare(dma.descriptor_head(), queue, AirFormat::Ht)
    }

    fn prepare_bound_he_tx(
        &mut self,
        dma: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        queue: MacTxQueueIndex,
        _program: MacHeTxProgram,
    ) -> Result<(), MacTxPrepareRefused> {
        self.prepare(dma.descriptor_head(), queue, AirFormat::He)
    }

    /// `TX-CHAIN`, `TX-PSDU`: the MAC follows the chain from the queue's
    /// head; the model takes its bytes as they stand at the start.
    fn start_bound_tx<D: CommitTxDma>(
        &mut self,
        mut publication: D,
        queue: MacTxQueueIndex,
        handed_over: impl FnOnce(),
    ) -> Result<(), MacTxPrepareRefused> {
        let index = queue.get() as u8;
        let head = publication.descriptor_head().bits();
        let (named, format) = self.atoms.queue(index, |place| (place.named, place.format));
        let (Some(named), Some(format)) = (named, format) else {
            self.atoms.broke(RuleBreak::NotNamed);
            return Err(MacTxPrepareRefused::Fault(
                MacTxPrepareFault::ChainNotNamed { named: 0 },
            ));
        };
        if named != head {
            self.atoms.broke(RuleBreak::NotNamed);
            return Err(MacTxPrepareRefused::Fault(
                MacTxPrepareFault::ChainNotNamed { named },
            ));
        }
        let mut mpdus = Vec::new();
        let mut read = 0_usize;
        let mut metadata = None;
        let chain = publication.read_chain(&mut |bytes| {
            read = read.saturating_add(bytes.len());
            match psdu(bytes) {
                Ok(mpdu) => mpdus.push(mpdu),
                Err(rule) => metadata = Some(rule),
            }
        });
        if let Err(unreadable) = chain {
            self.atoms.broke(RuleBreak::Chain(unreadable));
        }
        if let Some(rule) = metadata {
            self.atoms.broke(rule);
        }
        handed_over();
        publication.commit(|_started| {});
        self.atoms.queue(index, |place| {
            place.on_air = true;
            place.ended = false;
        });
        self.atoms.with(|state| {
            state.counts.ppdus = state.counts.ppdus.saturating_add(1);
            state.counts.tx_dma_bytes = state
                .counts
                .tx_dma_bytes
                .saturating_add(u32::try_from(read).unwrap_or(u32::MAX));
            state.air.push(AirFrame {
                queue: index,
                format,
                mpdus,
            });
        });
        Ok(())
    }

    fn take_tx_completion(&mut self, queue: MacTxQueueIndex) -> Option<MacTxCompletionObservation> {
        self.atoms
            .queue(queue.get() as u8, |place| place.completion.take())
    }

    fn take_block_ack_completion(
        &mut self,
        queue: MacTxQueueIndex,
    ) -> Option<MacHtAmpduCompletionObservation> {
        self.atoms
            .queue(queue.get() as u8, |place| place.block_ack.take())
    }

    fn begin_tx_timeout_abort(&mut self, _queue: MacTxQueueIndex, cause: MacTxAbortCause) -> bool {
        // The model raises no timeout edge: only the port's forced abort.
        if cause == MacTxAbortCause::TimeoutEdge {
            return false;
        }
        self.atoms.with(|state| {
            state.cca_forced = true;
            state.counts.aborts = state.counts.aborts.saturating_add(1);
        });
        true
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        queue: MacTxQueueIndex,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        let index = queue.get() as u8;
        let outcome = self.atoms.queue(index, |place| match reason {
            // `TX-DETACH`.
            MacTxDetachReason::Completed => Some(place.ended),
            // The model raises no collision edge.
            MacTxDetachReason::Collision => None,
            // An adverse input: this abort's end is the test's.
            MacTxDetachReason::Timeout => Some(!place.taken_by_air),
        });
        if reason == MacTxDetachReason::Timeout {
            self.atoms.with(|state| state.cca_forced = false);
        }
        match outcome {
            None => MacTxDetachOutcome::NoEvent,
            Some(false) => {
                if reason == MacTxDetachReason::Completed {
                    self.atoms.broke(RuleBreak::DetachOnAir { queue: index });
                }
                MacTxDetachOutcome::OnAir
            }
            Some(true) => {
                self.atoms.queue(index, |place| {
                    place.on_air = false;
                    place.ended = false;
                    place.named = None;
                    place.format = None;
                });
                MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                    expected_descriptor_head,
                )))
            }
        }
    }
}

/// `TX-PSDU`: the MPDU of a descriptor's used bytes: past the metadata
/// prefix, whose first word's low twenty bits count the PSDU with its FCS.
fn psdu(bytes: &[u8]) -> Result<Vec<u8>, RuleBreak> {
    let counted = bytes
        .first_chunk::<4>()
        .map_or(0, |word| u32::from_le_bytes(*word) & METADATA_LENGTH) as usize;
    let mpdu = counted
        .checked_sub(FCS)
        .and_then(|length| bytes.get(TX_PREFIX..)?.get(..length));
    match mpdu {
        Some(mpdu) if TX_PREFIX.saturating_add(counted) == bytes.len() => Ok(mpdu.to_vec()),
        _ => Err(RuleBreak::Metadata {
            used: bytes.len(),
            counted,
        }),
    }
}

impl HtAmpduHardware for ModelRegisters {
    fn prepare_he_trigger_based_queue(
        &mut self,
        _policy: MacHeTbTidLimit,
        _reservation: MacHeTbLinkReservation,
        _tid: MacHeTid,
        _mpdu_lengths: &[u16],
        _queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        self.atoms.unmodelled("prepare_he_trigger_based_queue");
        Err(MacHeTbProgramError::TidNotEligible)
    }

    fn clear_he_trigger_based_queue(&mut self, _reservation: MacHeTbLinkReservation) {
        self.atoms.unmodelled("clear_he_trigger_based_queue");
    }
}

impl CcmpKeyHardware for ModelRegisters {
    fn configure_ccmp_association(&mut self, _interface: MacInterface, _mode: MacSppMode) {
        self.atoms.with(|_| {});
    }
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        self.atoms.with(|state| {
            if state.keys.iter().any(|(slot, _)| *slot == index) {
                return MacKeyInstallOutcome::Occupied;
            }
            state.keys.push((index, identity));
            MacKeyInstallOutcome::Installed
        })
    }
    fn clear_ccmp_entry(&mut self, index: u8) {
        self.atoms
            .with(|state| state.keys.retain(|(slot, _)| *slot != index));
    }
}

impl RxBlockAckHardware for ModelRegisters {
    fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        let agreement = agreement.validate()?;
        self.atoms
            .with(|state| state.block_acks.push(agreement.hardware_index));
        Ok(())
    }
    fn clear_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        self.atoms
            .with(|state| state.block_acks.retain(|index| *index != hardware_index));
        Ok(())
    }
    fn reset_rx_block_ack_window(
        &mut self,
        _hardware_index: u8,
        _tid: u8,
        _starting_sequence: SequenceNumber,
        _window: u16,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        self.atoms.unmodelled("reset_rx_block_ack_window");
        Ok(())
    }
    fn program_extra_softap_rx_block_ack(
        &mut self,
        _agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        self.atoms.unmodelled("program_extra_softap_rx_block_ack");
        Ok(())
    }
    fn clear_extra_softap_rx_block_ack(
        &mut self,
        _hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        self.atoms.unmodelled("clear_extra_softap_rx_block_ack");
        Ok(())
    }
    fn reset_extra_softap_rx_block_ack_window(
        &mut self,
        _hardware_index: u8,
        _starting_sequence: SequenceNumber,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        self.atoms
            .unmodelled("reset_extra_softap_rx_block_ack_window");
        Ok(())
    }
}

impl StaApRegisterHardware for ModelRegisters {
    fn apply_sta_ap_receive_registers(&mut self, _plan: MacStaApReceivePlan) {
        self.atoms.unmodelled("apply_sta_ap_receive_registers");
    }
    fn disable_station_receive_registers(&mut self) {
        self.atoms.with(|_| {});
    }
    fn disable_access_point_receive_registers(&mut self) {
        self.atoms.with(|_| {});
    }
    fn disable_all_role_receive_registers(&mut self) {
        self.atoms.with(|_| {});
    }
}

impl StaLinkRxPolicyHardware for ModelRegisters {
    fn apply_sta_link_policy(&mut self, _bssid: [u8; 6]) {
        self.atoms.with(|_| {});
    }
}

impl StaNoiseFloorHardware for ModelRegisters {
    /// An input the receive metadata carries; no rule rests on it.
    fn read_noise_floor_dbm(&self) -> i8 {
        -96
    }
}

impl StaEspNowRxPolicyHardware for ModelRegisters {
    fn apply_sta_esp_now_policy(&mut self, _bssid: [u8; 6]) {
        self.atoms.unmodelled("apply_sta_esp_now_policy");
    }
}

impl StaScanRxPolicyHardware for ModelRegisters {
    fn apply_sta_scan_policy(&mut self) {
        self.atoms.with(|_| {});
    }
}

impl MacSnifferHardware for ModelRegisters {
    fn configure_open_promiscuous_receive(&mut self) {
        self.atoms.with(|state| state.promiscuous = true);
    }
    fn disable_open_promiscuous_receive(&mut self) {
        self.atoms.with(|state| state.promiscuous = false);
    }
}

impl ApRxPolicyHardware for ModelRegisters {
    fn apply_ap_link_policy(&mut self, _access_point: [u8; 6]) {
        self.atoms.unmodelled("apply_ap_link_policy");
    }
    fn disable_ap_link_policy(&mut self) {
        self.atoms.with(|_| {});
    }
}

impl ApTsfHardware for ModelRegisters {
    fn reset_and_start_access_point_tsf(&mut self, _write: AccessPointTsfWrite) {
        self.atoms.unmodelled("reset_and_start_access_point_tsf");
    }
    fn stop_access_point_tsf(&mut self, _write: AccessPointTsfWrite) {
        self.atoms.with(|_| {});
    }
}

impl StationTsfHardware for ModelRegisters {
    fn station_tsf(&mut self) -> u64 {
        self.atoms.with(|state| state.station_tsf)
    }
    fn set_station_tsf(&mut self, _write: StationTsfWrite, value: u64) {
        self.atoms.with(|state| state.station_tsf = value);
    }
}

impl StationTbttHardware for ModelRegisters {
    fn start_station_tbtt(&mut self, schedule: StaTbttSchedule) {
        self.atoms.with(|state| state.tbtt = Some(schedule));
    }
    fn stop_station_tbtt(&mut self) {
        self.atoms.with(|state| state.tbtt = None);
    }
}

impl TxGateHardware for ModelRegisters {
    fn set_power_save_tx_block(&mut self, blocked: bool) {
        self.atoms.with(|state| state.tx_blocked = blocked);
    }
}

impl crate::lower_mac::StaHeHardware for ModelRegisters {
    fn connect_he20_bss(
        &mut self,
        color: oer_chips_esp32s31_hal::types::MacHeBssColor,
        enabled: bool,
        partial: bool,
    ) {
        self.atoms
            .with(|state| state.he_bss = Some((color, enabled, partial)));
    }
    fn install_he20_association(
        &mut self,
        association: oer_chips_esp32s31_hal::types::MacHe20Association,
    ) {
        self.atoms.with(|state| {
            // The association writes the response's color.
            let peer = association.peer;
            state.he_bss = Some((
                peer.bss_color,
                peer.bss_color_enabled,
                peer.partial_bss_color,
            ));
            state.he = Some(association);
        });
    }
    fn clear_he20_association(&mut self) {
        self.atoms.with(|state| {
            state.he_bss = None;
            state.he = None;
        });
    }
}

impl RxBeaconPriorityHardware for ModelRegisters {
    fn set_rx_beacon_pti(&mut self, _pti: MacPti) {
        self.atoms.with(|_| {});
    }
    fn clear_rx_beacon_pti(&mut self) {
        self.atoms.with(|_| {});
    }
}

/// The calibrated transmit power of a model: an input no rule rests on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelPower;

impl WifiTxPowerProfile for ModelPower {
    fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
        WifiTxPowerPair {
            primary: 8,
            alternate: 8,
        }
    }
}

/// The stopped radio's owner over the model: where the chip has
/// `WifiStopped`. The model has no radio system: a start on another channel
/// is a retune it refuses, with the radio changed in nothing.
pub struct ModelFrontier {
    registers: ModelRegisters,
    channel: WifiChannel,
}

impl ModelFrontier {
    pub const fn new(registers: ModelRegisters, channel: WifiChannel) -> Self {
        Self { registers, channel }
    }

    pub const fn channel(&self) -> WifiChannel {
        self.channel
    }
}

impl Frontier for ModelFrontier {
    type Registers = ModelRegisters;
    type Space = HostDma;
    type Calibrated = ModelPower;
    type Context = WifiChannel;
    type Cell = ();
    type Retune = Option<Self>;
    type Failed = Infallible;
    fn into_running(self) -> (ModelRegisters, WifiChannel) {
        (self.registers, self.channel)
    }
    fn into_stopped(registers: ModelRegisters, channel: WifiChannel) -> Self {
        Self { registers, channel }
    }
    fn current_channel(context: &WifiChannel) -> WifiChannel {
        *context
    }
    fn start_retune(
        self,
        (): (),
        _channel: Permitted<RegulatedChannel<WifiChannel>>,
        _ceiling: TxCeiling,
    ) -> Option<Self> {
        self.registers.atoms.unmodelled("start_retune");
        Some(self)
    }
    fn poll_retune(
        retune: &mut Option<Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> Poll<RetuneEnd<Self>> {
        Poll::Ready(match retune.take() {
            Some(stopped) => RetuneEnd::Unchanged {
                stopped,
                cell: (),
                cause: WifiRetuneError::NotStopped(maintenance::Error::RxWalkerEnabled),
            },
            None => RetuneEnd::Abandoned,
        })
    }
}

/// The handler's lowering of the doorbell.
#[derive(Default)]
pub struct ModelLower;

impl DoorbellAcknowledge for ModelLower {
    fn acknowledge(&mut self) {}
}

type ModelPart = WifiIrqPart<ModelLower, (), MacInterruptModel, PowerInterruptModel>;

/// The interrupt line: the banks, and the handler a level runs at once, as
/// it preempts the tasks (`IRQ-LINE`).
pub struct ModelLine<X: SignalExecutor + 'static> {
    part: RefCell<ModelPart>,
    cell: &'static HandlerCell<WifiIrqLine<X>>,
    requests: &'static WifiIrqRequests,
    atoms: Atoms,
}

impl<X: SignalExecutor + 'static> ModelLine<X> {
    /// A line over closed banks, as the chip's handler starts.
    pub fn new(
        cell: &'static HandlerCell<WifiIrqLine<X>>,
        requests: &'static WifiIrqRequests,
        atoms: Atoms,
    ) -> Self {
        let epoch = crate::mac::irq::epoch::InterruptEpoch::new(
            MacInterruptModel::masked(),
            PowerInterruptModel::masked(),
        );
        Self {
            part: RefCell::new(WifiIrqPart::for_host_test(epoch, (), ModelLower)),
            cell,
            requests,
            atoms,
        }
    }

    /// `RX-EDGE`, `TX-DONE`: the MAC latches `events`; an enabled one
    /// asserts the line.
    pub fn latch(&self, events: MacInterruptObservation) {
        self.part
            .borrow_mut()
            .epoch_mut()
            .banks_mut()
            .0
            .latch(events);
        self.serve();
    }

    /// The handler, while the line stays asserted (`IRQ-LINE`).
    pub fn serve(&self) {
        for _ in 0..LINE_ENTRIES {
            let asserted = {
                let part = self.part.borrow();
                self.requests.is_pending() || part.epoch().banks().0.line_asserted()
            };
            if !asserted {
                return;
            }
            self.atoms.count(|counts| {
                counts.interrupt_entries = counts.interrupt_entries.saturating_add(1);
            });
            let served = crate::irq::service_wifi_line(self.cell, &mut self.part.borrow_mut());
            if let Err(crate::mac::irq::Fault::FatalAssertion { raw }) = served {
                self.atoms.broke(RuleBreak::Fatal(raw));
                return;
            }
        }
        self.atoms.broke(RuleBreak::LineStuck);
    }

    /// Whether every source of the MAC bank is masked.
    pub fn masked(&self) -> bool {
        let part = self.part.borrow();
        let mac = part.epoch().banks().0;
        !mac.rx_delivery_enabled() && !mac.other_sources_enabled()
    }

    /// Whether the MAC bank delivers RX events.
    pub fn rx_delivery_enabled(&self) -> bool {
        self.part.borrow().epoch().banks().0.rx_delivery_enabled()
    }

    /// Whether the MAC bank latched anything.
    pub fn latched(&self) -> bool {
        !self.part.borrow().epoch().banks().0.latched().is_empty()
    }
}

/// The radio doorbell of a model: a ring is an interrupt, at once.
pub struct ModelDoorbell<X: SignalExecutor + 'static>(pub Rc<ModelLine<X>>);

impl<X: SignalExecutor + 'static> RadioDoorbell for ModelDoorbell<X> {
    const HANDLER: HandlerContext = HandlerContext { level: 3, hart: 0 };
    const EXECUTOR: HandlerContext = HandlerContext { level: 2, hart: 0 };
    fn ring(&self) {
        self.0.atoms.count(|counts| {
            counts.doorbell_rings = counts.doorbell_rings.saturating_add(1);
        });
        self.0.serve();
    }
}

/// An S31 RX-control prefix (`RX-PREFIX`): the fixed 0x38 bytes and the
/// dynamic tail before the MPDU at 0x40. `rx_state` and the lengths are a
/// test's inputs.
#[derive(Clone, Copy)]
pub struct RxPrefix {
    pub rssi: i8,
    /// The raw MAC local time at 0x0c.
    pub stamp: u32,
    /// The RXEND state at 0x08.
    pub rxend_state: u8,
    /// The tail's RX state: `0xf5` MIC failure, `0xc6` quarantined.
    pub rx_state: u8,
    /// `sig_len` instead of the MPDU length plus the FCS.
    pub signal_length: Option<u16>,
    /// The dump length instead of `sig_len` plus the FCS.
    pub dump_length: Option<u16>,
}

impl RxPrefix {
    /// A legacy OFDM prefix of a cleanly received frame.
    pub const fn new() -> Self {
        Self {
            rssi: -50,
            stamp: 0x1000_0000,
            rxend_state: 0,
            rx_state: 0,
            signal_length: None,
            dump_length: None,
        }
    }

    /// The received bytes of a unit carrying `mpdu` behind this prefix.
    pub fn unit(&self, mpdu: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; PUBLIC_HEADER_SIZE + mpdu.len()];
        let length = u16::try_from(mpdu.len().saturating_add(FCS)).unwrap_or(u16::MAX);
        let signal_length = self.signal_length.unwrap_or(length);
        let dump_length = self
            .dump_length
            .unwrap_or(signal_length.saturating_add(FCS as u16));
        let fixed: &mut [(usize, &[u8])] = &mut [
            (0, &[self.rssi as u8]),
            (8, &[self.rxend_state]),
            (0x0c, &self.stamp.to_le_bytes()),
            (0x25, &[1 << 4]),
            (0x38, &signal_length.to_le_bytes()),
            (0x3a, &dump_length.to_le_bytes()),
            (0x3c, &[self.rx_state]),
            (PUBLIC_HEADER_SIZE, mpdu),
        ];
        for (at, field) in fixed.iter() {
            if let Some(place) = bytes
                .get_mut(*at..)
                .and_then(|rest| rest.get_mut(..field.len()))
            {
                place.copy_from_slice(field);
            }
        }
        bytes
    }
}

impl Default for RxPrefix {
    fn default() -> Self {
        Self::new()
    }
}

/// The DMA engine's write port on the ring (`RX-LAND`, `RX-EDGE`).
pub struct ModelRx<const N: usize, const B: usize, const S: usize> {
    storage: &'static RxDmaStorage<N, B, S, HostDma>,
    walker: &'static RefCell<RxWalkerModel>,
    atoms: Atoms,
}

impl<const N: usize, const B: usize, const S: usize> ModelRx<N, B, S> {
    pub const fn new(
        storage: &'static RxDmaStorage<N, B, S, HostDma>,
        walker: &'static RefCell<RxWalkerModel>,
        atoms: Atoms,
    ) -> Self {
        Self {
            storage,
            walker,
            atoms,
        }
    }

    /// Land `mpdu` behind `prefix`. A unit that landed returns the event its
    /// edge latches (`RX-EDGE`); one the walker did not take, none.
    ///
    /// # Errors
    ///
    /// The unit does not fit the armed buffer; nothing landed.
    pub fn land(
        &self,
        prefix: &RxPrefix,
        mpdu: &[u8],
    ) -> Result<(RxWalkerLanding, Option<MacInterruptObservation>), HostDmaAccessError> {
        let unit = prefix.unit(mpdu);
        let landing = self.storage.land_unit(self.walker, &unit)?;
        let RxWalkerLanding::Landed(_) = landing else {
            return Ok((landing, None));
        };
        self.atoms.with(|state| {
            state.counts.units_landed = state.counts.units_landed.saturating_add(1);
            state.counts.rx_dma_bytes = state
                .counts
                .rx_dma_bytes
                .saturating_add(u32::try_from(unit.len()).unwrap_or(u32::MAX));
        });
        Ok((
            landing,
            Some(MacInterruptObservation::from_semantic_events(
                MacInterruptEvents::RX_SUCCESS,
                false,
                false,
            )),
        ))
    }

    /// The units the ring has out of its descriptors.
    pub fn detached(&self) -> usize {
        self.storage.detached_buffer_count()
    }
}

/// `value`, placed for the rest of the test's process, as a static is.
pub fn leak<T>(value: T) -> &'static mut T {
    Box::leak(Box::new(value))
}
