//! Production A-MPDU completion and retry retention compared with
//! `ppResortTxAMPDU`.
//!
//! One entry commits the given encoded MPDUs to the retained A-MPDU owner,
//! publishes the aggregate through [`CompletionDouble`], observes one
//! BlockAck completion through the production retry state and, when that
//! state retains the aggregate, applies the production retry retention, which
//! sets the Retry bit of every retained MPDU in its own DMA backing. The
//! layout entry reports where those backings hold each MPDU, so the vendor
//! side can compare the same bytes.

use oer_esp32s31_hal::types::{
    MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
    MacHeTriggerTxQueueSnapshot, MacHtAmpduCompletionObservation, MacHtTxProgram,
    MacLegacyTxProgram, MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason,
    MacTxQueueDetached,
};
use oer_esp32s31_ieee80211_dma::tx_ampdu_storage::{AmpduDmaStorage, RetainedAmpduDmaStorage};
use oer_esp32s31_ieee80211_mac::tx::ampdu::{
    AmpduFrameLayout, AmpduFrameSize, AmpduRepublication, HtAmpduFrameRequest, HtAmpduHardware,
    HtAmpduTxResources, HtAmpduTxStorage, RetainedDmaAmpduTx, TX_AMPDU_METADATA_SIZE,
};
use oer_esp32s31_ieee80211_mac::tx::runtime::{
    AmpduRetryDecision, AmpduRetryPolicy, AmpduRetryState,
};
use oer_esp32s31_ieee80211_mac::tx::{
    HtAmpduTxConfig, HtChannelWidth, HtGuardInterval, HtMcs, HtRate, LegacyTxQueue, TxHardware,
};
use oer_ieee80211_lower_mac::{
    GenerationSource, Ieee80211ClockGeneration, Ieee80211Instant, Ieee80211Stamp,
};
use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_memory::{
    HardwareOwnedTxDma, PinnedDmaTxPool, PinnedDmaTxPoolHandle, PinnedDmaTxRadioLease,
    PreparedTxDma,
};
use oer_time::RadioDuration;

/// MPDUs one compared aggregate may hold.
const SLOTS: usize = 8;
/// Bytes of one encoded MPDU, the bytes the network stack publishes with
/// the private A-MPDU metadata prefix, and the pool slot capacity, which
/// leaves room for the trailer hardware appends.
const MPDU_BYTES: usize = 32;
const PUBLISHED_BYTES: usize = TX_AMPDU_METADATA_SIZE + MPDU_BYTES;
const FRAME_CAPACITY: usize = 64;
/// Trailer bytes hardware appends to each MPDU.
const HARDWARE_MIC_BYTES: u8 = 0;
/// Output words of the resort entry.
const OUTPUT_DECISION: usize = 0;
const OUTPUT_MASK: usize = 1;
const OUTPUT_FIRST_SEQUENCE: usize = 2;
const OUTPUT_SUBFRAMES: usize = 3;
/// Decisions the resort entry reports.
const DECISION_RETAIN: u32 = 1;
const DECISION_FINISH: u32 = 2;
const DECISION_REPUBLISH: u32 = 3;
const DECISION_TRIGGER: u32 = 4;
const DECISION_UNAGGREGATE: u32 = 5;
const DECISION_REQUEST_BLOCK_ACK: u32 = 6;
/// Failure codes: invalid inputs, then the production step that failed.
const INVALID_INPUT: u32 = 1;
const BEGIN_FAILED: u32 = 2;
const COMMIT_FAILED: u32 = 3;
const SUBMIT_FAILED: u32 = 4;
const COMPLETION_FAILED: u32 = 5;
const NO_COMPLETION: u32 = 6;
const RETAIN_FAILED: u32 = 7;
const RESOURCES_FAILED: u32 = 8;
/// Completion status of an ordinary aggregate that received its response.
const STATUS_COMPLETED: u8 = 0;
/// The time the probe commits an aggregate, from which its MSDUs age.
const COMMITTED_MICROS: u64 = 0;
/// The MAC clock generation every probe sample belongs to: the first of
/// the probe's one relation.
const CLOCK_GENERATION: Ieee80211ClockGeneration = GenerationSource::new(1).current();

/// A MAC clock sample `micros` after the probe's epoch.
fn radio_stamp(micros: u64) -> Ieee80211Stamp {
    Ieee80211Stamp {
        at: Ieee80211Instant::from_micros(micros),
        generation: CLOCK_GENERATION,
    }
}

type Pool = PinnedDmaTxPool<FRAME_CAPACITY, 0, 0, SLOTS>;
type Backing = PinnedDmaTxRadioLease<'static, FRAME_CAPACITY, 0, 0>;

#[derive(bytemuck::Zeroable)]
struct ProbeCell<T>(core::cell::UnsafeCell<T>);

// SAFETY: Blobray executes this probe image on one thread and invokes its
// exported entries serially.
unsafe impl<T> Sync for ProbeCell<T> {}

oer_memory::zeroed_static! {
    static POOL: ProbeCell<Pool> = zeroed in ".dma.bss.ampdu_resort";
}
static METADATA: ProbeCell<HtAmpduTxStorage<SLOTS, 0>> =
    ProbeCell(core::cell::UnsafeCell::new(HtAmpduTxStorage::new()));
oer_memory::zeroed_static! {
    static DMA: ProbeCell<AmpduDmaStorage<SLOTS, 0>> = zeroed in ".dma.bss.ampdu_resort";
}
static RETENTION: ProbeCell<RetainedAmpduDmaStorage<Backing, SLOTS>> =
    ProbeCell(core::cell::UnsafeCell::new(RetainedAmpduDmaStorage::new()));

/// The pinned pool; each entry runs on a freshly loaded image.
fn pool() -> PinnedDmaTxPoolHandle<FRAME_CAPACITY, 0, 0, SLOTS> {
    // SAFETY: the single-threaded image takes the pool once per entry.
    Pool::pin_static(unsafe { &mut *POOL.0.get() })
}

/// A hardware double: publication succeeds, the queue detaches at once, and
/// the one completion carries the given BlockAck result.
struct CompletionDouble {
    completion: Option<MacHtAmpduCompletionObservation>,
}

impl TxHardware for CompletionDouble {
    fn prepare_bound_legacy_tx(
        &mut self,
        _: &dyn PreparedTxDma,
        _: u8,
        _: MacLegacyTxProgram,
    ) -> bool {
        false
    }

    fn start_bound_legacy_tx(&mut self, _: &dyn HardwareOwnedTxDma, _: u8) {}

    fn prepare_bound_ht_tx(&mut self, _: &dyn PreparedTxDma, _: u8, _: MacHtTxProgram) -> bool {
        true
    }

    fn take_tx_completion(&mut self, _: u8) -> Option<MacTxCompletionObservation> {
        None
    }

    fn begin_tx_timeout_abort(&mut self, _: u8) -> bool {
        false
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _: u8,
        descriptor_head: u32,
        _: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_validation(
            descriptor_head,
        )))
    }

    fn take_block_ack_completion(&mut self, _: u8) -> Option<MacHtAmpduCompletionObservation> {
        self.completion.take()
    }
}

impl HtAmpduHardware for CompletionDouble {
    fn prepare_he_trigger_based_queue(
        &mut self,
        _: MacHeTbTidLimit,
        _: MacHeTbLinkReservation,
        _: MacHeTid,
        _: &[u16],
        _: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        Err(MacHeTbProgramError::LengthCountMismatch)
    }

    fn clear_he_trigger_based_queue(&mut self, _: MacHeTbLinkReservation) {}
}

oer_probe_macros::probe! {
    /// Each pool slot's MPDU address, written to `output`: where the resort
    /// entry keeps the encoded MPDU of that position.
    ///
    /// # Safety
    /// `output` must point to `SLOTS` writable words.
    pub unsafe fn open_libpp_ampdu_trace_layout(output: *mut u32) -> u32 {
        let pool = pool();
        for index in 0..SLOTS as u8 {
            let (slot, address) = pool
                .claim_network(index)
                .publish(PUBLISHED_BYTES, |bytes| {
                    bytes[TX_AMPDU_METADATA_SIZE..].as_ptr() as u32
                });
            drop(pool.claim_radio(slot));
            // SAFETY: the caller provides `SLOTS` output words.
            unsafe { output.add(usize::from(index)).write(address) };
        }
        0
    }
}

oer_probe_macros::probe! {
    /// Commit `count` MPDUs of `MPDU_BYTES` each from `frames`, the first
    /// carrying `first_sequence`, publish them as one HT A-MPDU and observe
    /// one completion whose BlockAck starts at `starting_sequence` with
    /// `bitmap_low` and `bitmap_high`, received when `received` is nonzero,
    /// `elapsed_micros` after the commit. The retry state ages MSDUs after
    /// `lifetime_micros` and keeps a single missing MPDU when
    /// `retain_single` is nonzero. When `trigger_flow` is nonzero the
    /// completion is instead an acknowledgement timeout of a queue in
    /// Trigger flow with no Trigger-based packets pending, which ends the
    /// aggregate through the vendor's Trigger-based success. The BlockAck
    /// agreement is operational when `block_ack_operational` is nonzero. A
    /// retained
    /// aggregate is compacted with the Retry bit set. Writes the decision,
    /// its retry mask, the next first sequence and subframe count to
    /// `output`; returns zero, or the step that failed.
    ///
    /// # Safety
    /// `frames` must point to `count * MPDU_BYTES` readable bytes and
    /// `output` to four writable words.
    pub unsafe fn open_libpp_ampdu_trace_resort(
        frames: *const u8,
        count: u32,
        first_sequence: u32,
        starting_sequence: u32,
        bitmap_low: u32,
        bitmap_high: u32,
        received: u32,
        lifetime_micros: u32,
        elapsed_micros: u32,
        retain_single: u32,
        trigger_flow: u32,
        block_ack_operational: u32,
        output: *mut u32,
    ) -> u32 {
        let count = count as usize;
        let Some(first) = u16::try_from(first_sequence).ok().and_then(SequenceNumber::new) else {
            return INVALID_INPUT;
        };
        if count == 0 || count > SLOTS {
            return INVALID_INPUT;
        }
        let pool = pool();
        // SAFETY: each static is taken once for this entry's owner.
        let resources = unsafe {
            HtAmpduTxResources::pin_static(&mut *METADATA.0.get(), &mut *DMA.0.get())
        };
        let Ok(resources) = resources else {
            return RESOURCES_FAILED;
        };
        // SAFETY: as above, the retention storage has no other reference.
        let mut owner = RetainedDmaAmpduTx::new(resources, unsafe { &mut *RETENTION.0.get() });
        let Ok(cookie) = owner.begin() else {
            return BEGIN_FAILED;
        };
        let rate = HtRate::new(HtMcs::Mcs0, HtGuardInterval::Long800Ns, HtChannelWidth::Mhz20);
        let Some(layout) =
            AmpduFrameLayout::new(0, AmpduFrameSize::new(MPDU_BYTES, HARDWARE_MIC_BYTES))
        else {
            return INVALID_INPUT;
        };
        for index in 0..count {
            // The network stack hands the encoded MPDU to the pool.
            let (slot, ()) = pool.claim_network(index as u8).publish(PUBLISHED_BYTES, |bytes| {
                // SAFETY: the caller provides `count * MPDU_BYTES` bytes.
                let source =
                    unsafe { core::slice::from_raw_parts(frames.add(index * MPDU_BYTES), MPDU_BYTES) };
                bytes[TX_AMPDU_METADATA_SIZE..].copy_from_slice(source);
            });
            let request = HtAmpduFrameRequest::new(layout, 0, rate);
            if owner.commit_ht(cookie, pool.claim_radio(slot), request).is_err() {
                return COMMIT_FAILED;
            }
        }
        let Ok(aggregate) = owner.prepared_aggregate(cookie) else {
            return COMMIT_FAILED;
        };
        let Some(config) = HtAmpduTxConfig::new(rate, aggregate.bytes, aggregate.subframes) else {
            return SUBMIT_FAILED;
        };
        let bitmap = u64::from(bitmap_high) << 32 | u64::from(bitmap_low);
        let mut hardware = CompletionDouble {
            completion: Some(if trigger_flow != 0 {
                MacHtAmpduCompletionObservation::new_validation(
                    MacTxCompletionObservation::new_validation(STATUS_ACK_TIMEOUT, 0)
                        .with_trigger_flow_model(true)
                        .with_trigger_packet_counts_model(0, false, 0),
                    0,
                    0,
                    0,
                    false,
                )
            } else {
                MacHtAmpduCompletionObservation::new_validation(
                    MacTxCompletionObservation::new_validation(STATUS_COMPLETED, 0),
                    0,
                    starting_sequence as u16,
                    bitmap,
                    received != 0,
                )
            }),
        };
        if owner
            .submit(&mut hardware, cookie, LegacyTxQueue::BestEffort, config)
            .is_err()
        {
            return SUBMIT_FAILED;
        }
        let policy = AmpduRetryPolicy {
            lifetime: RadioDuration::from_micros(u64::from(lifetime_micros)),
            retain_single_mpdu: retain_single != 0,
        };
        let Ok(mut retry) =
            AmpduRetryState::<SLOTS>::new(first, count as u8, policy, radio_stamp(COMMITTED_MICROS))
        else {
            return INVALID_INPUT;
        };
        let now = radio_stamp(COMMITTED_MICROS + u64::from(elapsed_micros));
        let observed = match owner.observe_retry_completion(
            &mut hardware,
            cookie,
            &mut retry,
            now,
            block_ack_operational != 0,
        ) {
            Ok(Some(observed)) => observed,
            Ok(None) => return NO_COMPLETION,
            Err(_) => return COMPLETION_FAILED,
        };
        let decision = match observed.decision {
            AmpduRetryDecision::RetainAggregate { .. } => DECISION_RETAIN,
            AmpduRetryDecision::Finish { .. } => DECISION_FINISH,
            AmpduRetryDecision::RepublishUnchanged { .. } => DECISION_REPUBLISH,
            AmpduRetryDecision::FinishTriggerFlow => DECISION_TRIGGER,
            AmpduRetryDecision::Unaggregate { .. } => DECISION_UNAGGREGATE,
            AmpduRetryDecision::RequestBlockAck { .. } => DECISION_REQUEST_BLOCK_ACK,
        };
        if decision == DECISION_RETAIN
            && owner
                .retain_for_ampdu_retry(
                    cookie,
                    observed.decision.retry_mask(),
                    AmpduRepublication::Retransmission,
                )
                .is_err()
        {
            return RETAIN_FAILED;
        }
        // SAFETY: the caller provides four output words.
        unsafe {
            output.add(OUTPUT_DECISION).write(decision);
            output.add(OUTPUT_MASK).write(observed.decision.retry_mask());
            output
                .add(OUTPUT_FIRST_SEQUENCE)
                .write(u32::from(retry.current_first_sequence().get()));
            output
                .add(OUTPUT_SUBFRAMES)
                .write(u32::from(retry.current_subframes()));
        }
        // The comparison ends with the backings retained: releasing them is
        // not part of the compared transaction.
        core::mem::forget(owner);
        0
    }
}

/// Completion status of an acknowledgement timeout: no BlockAck arrived.
const STATUS_ACK_TIMEOUT: u8 = 5;
/// Decision of a step with no aggregate in flight.
const NO_AGGREGATE: u32 = 9;

type TimeoutOwner = RetainedDmaAmpduTx<'static, Backing, SLOTS, 0>;

/// The aggregate a timeout sequence republishes across warm phases, its
/// cookie and retry state, and the publication it was built with.
struct TimeoutSequence {
    owner: TimeoutOwner,
    cookie: oer_esp32s31_ieee80211_mac::tx::TxCookie,
    retry: AmpduRetryState<SLOTS>,
    rate: HtRate,
}

static SEQUENCE: ProbeCell<Option<TimeoutSequence>> = ProbeCell(core::cell::UnsafeCell::new(None));

/// A double whose publication succeeds and whose next completion carries
/// `status` without a BlockAck.
fn timeout_double(status: u8) -> CompletionDouble {
    CompletionDouble {
        completion: Some(MacHtAmpduCompletionObservation::new_validation(
            MacTxCompletionObservation::new_validation(status, 0),
            0,
            0,
            0,
            false,
        )),
    }
}

oer_probe_macros::probe! {
    /// Commit `count` MPDUs of `MPDU_BYTES` each from `frames`, the first
    /// carrying `first_sequence`, and publish them as one HT A-MPDU whose
    /// retry state ages MSDUs after `lifetime_micros`; the sequence stays in
    /// flight for the step entry, whose completions arrive at the commit
    /// time. Returns zero, or the step that failed.
    ///
    /// # Safety
    /// `frames` must point to `count * MPDU_BYTES` readable bytes.
    pub unsafe fn open_libpp_ampdu_trace_timeout_begin(
        frames: *const u8,
        count: u32,
        first_sequence: u32,
        lifetime_micros: u32,
    ) -> u32 {
        let count = count as usize;
        let Some(first) = u16::try_from(first_sequence).ok().and_then(SequenceNumber::new) else {
            return INVALID_INPUT;
        };
        if count == 0 || count > SLOTS {
            return INVALID_INPUT;
        }
        let pool = pool();
        // SAFETY: each static is taken once for this sequence's owner.
        let resources = unsafe {
            HtAmpduTxResources::pin_static(&mut *METADATA.0.get(), &mut *DMA.0.get())
        };
        let Ok(resources) = resources else {
            return RESOURCES_FAILED;
        };
        // SAFETY: as above, the retention storage has no other reference.
        let mut owner = RetainedDmaAmpduTx::new(resources, unsafe { &mut *RETENTION.0.get() });
        let Ok(cookie) = owner.begin() else {
            return BEGIN_FAILED;
        };
        let rate = HtRate::new(HtMcs::Mcs0, HtGuardInterval::Long800Ns, HtChannelWidth::Mhz20);
        let Some(layout) =
            AmpduFrameLayout::new(0, AmpduFrameSize::new(MPDU_BYTES, HARDWARE_MIC_BYTES))
        else {
            return INVALID_INPUT;
        };
        for index in 0..count {
            let (slot, ()) = pool.claim_network(index as u8).publish(PUBLISHED_BYTES, |bytes| {
                // SAFETY: the caller provides `count * MPDU_BYTES` bytes.
                let source = unsafe {
                    core::slice::from_raw_parts(frames.add(index * MPDU_BYTES), MPDU_BYTES)
                };
                bytes[TX_AMPDU_METADATA_SIZE..].copy_from_slice(source);
            });
            let request = HtAmpduFrameRequest::new(layout, 0, rate);
            if owner.commit_ht(cookie, pool.claim_radio(slot), request).is_err() {
                return COMMIT_FAILED;
            }
        }
        let Ok(aggregate) = owner.prepared_aggregate(cookie) else {
            return COMMIT_FAILED;
        };
        let Some(config) = HtAmpduTxConfig::new(rate, aggregate.bytes, aggregate.subframes) else {
            return SUBMIT_FAILED;
        };
        if owner
            .submit(
                &mut timeout_double(STATUS_ACK_TIMEOUT),
                cookie,
                LegacyTxQueue::BestEffort,
                config,
            )
            .is_err()
        {
            return SUBMIT_FAILED;
        }
        let policy = AmpduRetryPolicy {
            lifetime: RadioDuration::from_micros(u64::from(lifetime_micros)),
            retain_single_mpdu: false,
        };
        let Ok(retry) = AmpduRetryState::<SLOTS>::new(first, count as u8, policy, radio_stamp(COMMITTED_MICROS))
        else {
            return INVALID_INPUT;
        };
        // SAFETY: the single-threaded image owns the sequence cell here.
        unsafe {
            *SEQUENCE.0.get() = Some(TimeoutSequence {
                owner,
                cookie,
                retry,
                rate,
            })
        };
        0
    }
}

oer_probe_macros::probe! {
    /// Observe one completion with `status` and no BlockAck of the aggregate
    /// in flight: a retained aggregate is compacted with the Retry bit set,
    /// an unchanged one keeps its bytes, and both are published again; a
    /// finished one leaves the sequence. Returns the decision.
    pub fn open_libpp_ampdu_trace_timeout_step(status: u32) -> u32 {
        let Ok(status) = u8::try_from(status) else {
            return INVALID_INPUT;
        };
        // SAFETY: the single-threaded image owns the sequence cell here.
        let slot = unsafe { &mut *SEQUENCE.0.get() };
        let Some(sequence) = slot.as_mut() else {
            return NO_AGGREGATE;
        };
        let mut hardware = timeout_double(status);
        let observed = match sequence.owner.observe_retry_completion(
            &mut hardware,
            sequence.cookie,
            &mut sequence.retry,
            radio_stamp(COMMITTED_MICROS),
            true,
        ) {
            Ok(Some(observed)) => observed,
            Ok(None) => return NO_COMPLETION,
            Err(_) => return COMPLETION_FAILED,
        };
        let (retry_mask, republication, decision) = match observed.decision {
            AmpduRetryDecision::RetainAggregate { retry_mask } => (
                retry_mask,
                AmpduRepublication::Retransmission,
                DECISION_RETAIN,
            ),
            AmpduRetryDecision::RepublishUnchanged { retry_mask } => (
                retry_mask,
                AmpduRepublication::AfterProtectionFailure,
                DECISION_REPUBLISH,
            ),
            AmpduRetryDecision::Finish { .. } => {
                // The finished aggregate's backings stay retained: releasing
                // them is not part of the compared sequence.
                core::mem::forget(slot.take());
                return DECISION_FINISH;
            }
            AmpduRetryDecision::FinishTriggerFlow => return DECISION_TRIGGER,
            AmpduRetryDecision::Unaggregate { .. } => {
                core::mem::forget(slot.take());
                return DECISION_UNAGGREGATE;
            }
            // The kept aggregate now waits for its BlockAckReq's answer,
            // which is not part of the compared sequence.
            AmpduRetryDecision::RequestBlockAck { .. } => {
                core::mem::forget(slot.take());
                return DECISION_REQUEST_BLOCK_ACK;
            }
        };
        let Ok(retained) =
            sequence
                .owner
                .retain_for_ampdu_retry(sequence.cookie, retry_mask, republication)
        else {
            return RETAIN_FAILED;
        };
        let Some(config) =
            HtAmpduTxConfig::new(sequence.rate, retained.bytes, retained.subframes)
        else {
            return SUBMIT_FAILED;
        };
        if sequence
            .owner
            .submit(&mut hardware, sequence.cookie, LegacyTxQueue::BestEffort, config)
            .is_err()
        {
            return SUBMIT_FAILED;
        }
        decision
    }
}
