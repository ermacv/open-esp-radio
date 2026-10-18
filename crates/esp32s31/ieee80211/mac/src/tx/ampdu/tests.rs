//! Host/oracle tests for A-MPDU protocol, formatting and ownership.

use super::*;

use oer_esp32s31_hal::dma_static::{Placed, PlacedTxLease, PlacedTxPool};

/// A host pool, as if placed: its radio leases are the placed leases
/// A-MPDU takes.
fn host_pool<
    const CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const DEPTH: usize,
>(
    storage: &'static mut PinnedDmaTxPool<CAPACITY, HEADROOM, TRAILER, DEPTH>,
) -> PlacedTxPool<CAPACITY, HEADROOM, TRAILER, DEPTH> {
    PlacedTxPool::pin(Placed::host(storage))
}
mod length_budget;
use crate::tx::runtime::{AmpduRetryDecision, AmpduRetryPolicy, AmpduRetryState};
use crate::tx::{
    HeAmpduTxConfig, HeEdcaTxopLimit, HeRate, HtAmpduDensity, HtAmpduTxConfig, HtRate, TxHardware,
};
use oer_esp32s31_hal::types::{
    MacHeTbTidLimit, MacHeTid, MacHtAmpduCompletionObservation, MacHtTxProgram, MacLegacyTxProgram,
    MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason, MacTxQueueDetached,
};
use oer_memory::{HardwareOwnedTxDma, PinnedDmaTxPool, PinnedDmaTxRadioLease, PreparedTxDma};

struct CompletionHardware {
    completion: Option<MacHtAmpduCompletionObservation>,
    cleared: Option<MacHeTbLinkReservation>,
    trigger_snapshot: Option<MacHeTriggerTxQueueSnapshot>,
}

impl TxHardware for CompletionHardware {
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

    fn start_bound_ht_tx(&mut self, _: &dyn HardwareOwnedTxDma, _: u8) {}

    fn take_tx_completion(&mut self, _: u8) -> Option<MacTxCompletionObservation> {
        None
    }

    fn begin_tx_timeout_abort(&mut self, _: u8) -> bool {
        false
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _: u8,
        _: u32,
        _: MacTxDetachReason,
        _: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        MacTxDetachOutcome::NoEvent
    }

    fn take_block_ack_completion(&mut self, _: u8) -> Option<MacHtAmpduCompletionObservation> {
        self.completion.take()
    }
}

impl HtAmpduHardware for CompletionHardware {
    fn prepare_he_trigger_based_queue(
        &mut self,
        _: MacHeTbTidLimit,
        _: MacHeTbLinkReservation,
        _: MacHeTid,
        _: &[u16],
        _: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        self.trigger_snapshot
            .ok_or(MacHeTbProgramError::LengthCountMismatch)
    }

    fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation) {
        self.cleared = Some(reservation);
    }

    fn he_trigger_based_queue_snapshot(
        &self,
        _: MacHeTbLinkReservation,
    ) -> Option<MacHeTriggerTxQueueSnapshot> {
        self.trigger_snapshot
    }
}

struct DetachingCompletionHardware {
    completion: Option<MacHtAmpduCompletionObservation>,
    abort_requested: bool,
    detach_failed: bool,
    reject_publication: bool,
    control: Option<oer_esp32s31_hal::types::MacTxControlFrame>,
}

impl DetachingCompletionHardware {
    fn successful() -> Self {
        Self::with_bitmap(u64::MAX)
    }

    fn with_bitmap(bitmap: u64) -> Self {
        Self {
            abort_requested: false,
            detach_failed: false,
            reject_publication: false,
            control: None,
            completion: Some(MacHtAmpduCompletionObservation::new_model(
                MacTxCompletionObservation::new_model(0, 0),
                0,
                0,
                bitmap,
                true,
            )),
        }
    }
}

impl TxHardware for DetachingCompletionHardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _: &dyn PreparedTxDma,
        _: u8,
        _: MacLegacyTxProgram,
    ) -> bool {
        false
    }

    fn start_bound_legacy_tx(&mut self, _: &dyn HardwareOwnedTxDma, _: u8) {}

    fn prepare_bound_ht_tx(
        &mut self,
        _: &dyn PreparedTxDma,
        _: u8,
        program: MacHtTxProgram,
    ) -> bool {
        if !self.reject_publication {
            self.control = Some(program.control());
        }
        !self.reject_publication
    }

    fn start_bound_ht_tx(&mut self, _: &dyn HardwareOwnedTxDma, _: u8) {}

    fn prepare_bound_he_tx(
        &mut self,
        _: &dyn PreparedTxDma,
        _: u8,
        program: oer_esp32s31_hal::types::MacHeTxProgram,
    ) -> bool {
        if !self.reject_publication {
            self.control = Some(program.control());
        }
        !self.reject_publication
    }

    fn take_tx_completion(&mut self, _: u8) -> Option<MacTxCompletionObservation> {
        None
    }

    fn begin_tx_timeout_abort(&mut self, _: u8) -> bool {
        self.abort_requested = true;
        true
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _: u8,
        descriptor_head: u32,
        _: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        if self.detach_failed {
            return MacTxDetachOutcome::Failed;
        }
        MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(descriptor_head)))
    }

    fn take_block_ack_completion(&mut self, _: u8) -> Option<MacHtAmpduCompletionObservation> {
        self.completion.take()
    }
}

impl HtAmpduHardware for DetachingCompletionHardware {
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

fn frame_layout(
    dma_offset: usize,
    mpdu_length: usize,
    hardware_mic_length: u8,
) -> AmpduFrameLayout {
    AmpduFrameLayout::new(
        dma_offset,
        AmpduFrameSize::new(mpdu_length, hardware_mic_length),
    )
    .unwrap()
}

fn ht_frame_request(
    dma_offset: usize,
    mpdu_length: usize,
    hardware_mic_length: u8,
    empty_delimiters: u8,
    rate: HtRate,
) -> HtAmpduFrameRequest {
    HtAmpduFrameRequest::new(
        frame_layout(dma_offset, mpdu_length, hardware_mic_length),
        empty_delimiters,
        rate,
    )
}

fn he_frame_request(
    mpdu_length: usize,
    rate: HeRate,
    density: HtAmpduDensity,
) -> HeAmpduFrameRequest {
    HeAmpduFrameRequest::new(
        frame_layout(0, mpdu_length, 8),
        HeAmpduPolicy::new(rate, density, HeEdcaTxopLimit::DEFAULT),
    )
}

/// An HT rate whose vendor A-MPDU byte table adds no ceiling, so only the
/// slot, backing and peer limits admit a frame.
fn unlimited_ht_rate() -> HtRate {
    let rate = HtRate::new(
        crate::tx::HtMcs::Mcs7,
        crate::tx::HtGuardInterval::Short400Ns,
        crate::tx::HtChannelWidth::Mhz40,
    );
    assert_eq!(rate.vendor_ampdu_byte_limit(), None);
    rate
}

/// A pool of network buffers, the product's external A-MPDU backing.
fn frame_pool<const CAPACITY: usize, const DEPTH: usize>() -> PlacedTxPool<CAPACITY, 0, 0, DEPTH> {
    host_pool(std::boxed::Box::leak(std::boxed::Box::new(
        PinnedDmaTxPool::new(),
    )))
}

/// Encode one `length`-byte MPDU into pool slot `slot` behind the private
/// DMA metadata prefix and hand its buffer to the radio.
fn frame<const CAPACITY: usize, const DEPTH: usize>(
    pool: &PlacedTxPool<CAPACITY, 0, 0, DEPTH>,
    slot: u8,
    length: usize,
    encode: impl FnOnce(&mut [u8]),
) -> PlacedTxLease<PinnedDmaTxRadioLease<'static, CAPACITY, 0, 0>> {
    let (index, ()) = pool
        .claim_network(slot)
        .publish(TX_AMPDU_METADATA_SIZE + length, |bytes| {
            encode(&mut bytes[TX_AMPDU_METADATA_SIZE..]);
        });
    pool.claim_radio(index)
}

/// The first `length` bytes of a returned slot: the image the MAC left for
/// DMA, as the next network writer finds it.
fn released_slot<const CAPACITY: usize, const DEPTH: usize>(
    pool: &PlacedTxPool<CAPACITY, 0, 0, DEPTH>,
    slot: u8,
    length: usize,
) -> std::vec::Vec<u8> {
    let (lease, bytes) = pool
        .claim_network(slot)
        .try_publish(length, |bytes| Err::<(), _>(bytes.to_vec()))
        .err()
        .unwrap();
    lease.release();
    bytes
}

/// Publish the committed HT aggregate, then complete and detach it the way
/// the MAC reports a finished exchange.
fn complete_and_detach<B: oer_esp32s31_hal::dma_static::PlacedBacking, const SLOTS: usize>(
    owner: &mut RetainedDmaAmpduTx<'_, B, SLOTS>,
    cookie: TxCookie,
    rate: HtRate,
) {
    let aggregate = owner.prepared_aggregate(cookie).unwrap();
    let mut hardware = DetachingCompletionHardware::successful();
    owner
        .submit(
            &mut hardware,
            cookie,
            LegacyTxQueue::BestEffort,
            HtAmpduTxConfig::new(rate, aggregate.bytes, aggregate.subframes).unwrap(),
        )
        .unwrap();
    assert!(
        owner
            .acknowledge_completion(&mut hardware)
            .unwrap()
            .is_some()
    );
    owner.detach_completed(&mut hardware, cookie).unwrap();
}

#[test]
fn ampdu_frame_layout_rejects_unaligned_dma_prefix() {
    let frame_size = AmpduFrameSize::new(32, 8);
    assert_eq!(AmpduFrameLayout::new(1, frame_size), None);
    assert_eq!(
        AmpduFrameLayout::new(4, frame_size).unwrap().dma_offset(),
        4
    );
}

#[test]
fn retained_dma_owner_cancels_reserved_storage_before_releasing_backing() {
    let storage = HtAmpduTxStorage::<2>::new();
    let mut storage = core::pin::pin!(storage);
    let pool: PlacedTxPool<256, 0, 0, 1> = host_pool(std::boxed::Box::leak(std::boxed::Box::new(
        PinnedDmaTxPool::new(),
    )));
    let network = pool.claim_network(0);
    let (index, ()) = network.publish(TX_AMPDU_METADATA_SIZE + 32, |bytes| {
        bytes[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 32].fill(0x5a);
    });
    let backing = pool.claim_radio(index);
    let mut retention = RetainedAmpduDmaStorage::new();

    {
        let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
        let cookie = owner.begin().unwrap();
        owner
            .commit_ht(
                cookie,
                backing,
                ht_frame_request(
                    0,
                    32,
                    8,
                    0,
                    HtRate::new(
                        crate::tx::HtMcs::Mcs0,
                        crate::tx::HtGuardInterval::Long800Ns,
                        crate::tx::HtChannelWidth::Mhz20,
                    ),
                ),
            )
            .unwrap();
        assert_eq!(owner.held_backing_count(), 1);
    }

    assert_eq!(storage.state(), TxSlotState::Free);
    assert_eq!(pool.claim_network(0).release(), 0);
}

#[test]
fn rejected_referenced_commit_rolls_back_the_lower_lease() {
    let storage = HtAmpduTxStorage::<2>::new();
    let mut storage = core::pin::pin!(storage);
    let pool: PlacedTxPool<256, 0, 0, 1> = host_pool(std::boxed::Box::leak(std::boxed::Box::new(
        PinnedDmaTxPool::new(),
    )));
    let network = pool.claim_network(0);
    let (index, ()) = network.publish(TX_AMPDU_METADATA_SIZE + 32, |_| {});
    let backing = pool.claim_radio(index);

    {
        let mut retention = RetainedAmpduDmaStorage::new();
        let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
        let cookie = owner.begin().unwrap();
        assert_eq!(
            owner.commit_ht(
                cookie,
                backing,
                ht_frame_request(
                    256,
                    32,
                    8,
                    0,
                    HtRate::new(
                        crate::tx::HtMcs::Mcs0,
                        crate::tx::HtGuardInterval::Long800Ns,
                        crate::tx::HtChannelWidth::Mhz20,
                    ),
                ),
            ),
            Err(HtAmpduTxError::FrameTooLong)
        );
        assert_eq!(owner.held_backing_count(), 0);
        owner.cancel(cookie).unwrap();
    }

    assert_eq!(pool.claim_network(0).release(), 0);
}

#[test]
fn retained_dma_owner_quarantines_hardware_owned_backing_without_drop_panic() {
    let storage = HtAmpduTxStorage::<2>::new();
    let mut storage = core::pin::pin!(storage);
    let pool: PlacedTxPool<256, 0, 0, 1> = host_pool(std::boxed::Box::leak(std::boxed::Box::new(
        PinnedDmaTxPool::new(),
    )));
    let network = pool.claim_network(0);
    let (index, ()) = network.publish(TX_AMPDU_METADATA_SIZE + 32, |bytes| {
        bytes[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 32].fill(0x5a);
    });
    let backing = pool.claim_radio(index);

    {
        let mut retention = RetainedAmpduDmaStorage::new();
        let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
        let cookie = owner.begin().unwrap();
        let rate = HtRate::new(
            crate::tx::HtMcs::Mcs0,
            crate::tx::HtGuardInterval::Long800Ns,
            crate::tx::HtChannelWidth::Mhz20,
        );
        owner
            .commit_ht(cookie, backing, ht_frame_request(0, 32, 8, 0, rate))
            .unwrap();
        let aggregate = owner.prepared_aggregate(cookie).unwrap();
        owner
            .submit(
                &mut CompletionHardware {
                    completion: None,
                    cleared: None,
                    trigger_snapshot: None,
                },
                cookie,
                LegacyTxQueue::BestEffort,
                HtAmpduTxConfig::new(rate, aggregate.bytes, aggregate.subframes).unwrap(),
            )
            .unwrap();
        drop(owner);
    }

    assert_eq!(storage.state(), TxSlotState::ResetRequired);
    assert_eq!(pool.claimed_slots(), 1);
}

#[test]
fn retained_abort_releases_backing_only_after_successful_detach() {
    for detach_failed in [false, true] {
        let mut storage = core::pin::pin!(HtAmpduTxStorage::<2>::new());
        let pool: PlacedTxPool<256, 0, 0, 1> = host_pool(std::boxed::Box::leak(
            std::boxed::Box::new(PinnedDmaTxPool::new()),
        ));
        let (index, ()) = pool
            .claim_network(0)
            .publish(TX_AMPDU_METADATA_SIZE + 32, |_| {});
        let mut retention = RetainedAmpduDmaStorage::new();
        let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
        let cookie = owner.begin().unwrap();
        let rate = HtRate::new(
            crate::tx::HtMcs::Mcs0,
            crate::tx::HtGuardInterval::Long800Ns,
            crate::tx::HtChannelWidth::Mhz20,
        );
        owner
            .commit_ht(
                cookie,
                pool.claim_radio(index),
                ht_frame_request(0, 32, 8, 0, rate),
            )
            .unwrap();
        let aggregate = owner.prepared_aggregate(cookie).unwrap();
        let mut hardware = DetachingCompletionHardware::successful();
        hardware.detach_failed = detach_failed;
        owner
            .submit(
                &mut hardware,
                cookie,
                LegacyTxQueue::BestEffort,
                HtAmpduTxConfig::new(rate, aggregate.bytes, aggregate.subframes).unwrap(),
            )
            .unwrap();

        assert!(owner.begin_timeout_abort(&mut hardware, cookie).unwrap());
        assert!(hardware.abort_requested);
        assert_eq!(owner.held_backing_count(), 1);
        assert_eq!(pool.claimed_slots(), 1);
        // The caller retains the owner while waiting for the settle deadline.
        let result = owner.finish_timeout_abort(&mut hardware, cookie);
        if detach_failed {
            assert_eq!(result, Err(HtAmpduTxError::DetachFailed));
            drop(owner);
            assert_eq!(storage.state(), TxSlotState::ResetRequired);
            assert_eq!(pool.claimed_slots(), 1);
        } else {
            result.unwrap();
            assert_eq!(owner.held_backing_count(), 0);
            assert_eq!(pool.claimed_slots(), 0);
            assert_eq!(
                owner.finish_timeout_abort(&mut hardware, cookie),
                Err(HtAmpduTxError::Stale)
            );
            drop(owner);
            assert_eq!(storage.state(), TxSlotState::Free);
        }
    }
}

#[test]
fn retained_dma_owner_preserves_backing_identity_through_selective_retry() {
    let storage = HtAmpduTxStorage::<4>::new();
    let mut storage = core::pin::pin!(storage);
    let pool: PlacedTxPool<256, 0, 0, 4> = host_pool(std::boxed::Box::leak(std::boxed::Box::new(
        PinnedDmaTxPool::new(),
    )));
    let rate = HtRate::new(
        crate::tx::HtMcs::Mcs0,
        crate::tx::HtGuardInterval::Long800Ns,
        crate::tx::HtChannelWidth::Mhz20,
    );
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();

    for slot in 0..4 {
        let network = pool.claim_network(slot);
        let (index, ()) = network.publish(TX_AMPDU_METADATA_SIZE + 32, |bytes| {
            bytes[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 32].fill(slot);
            bytes[TX_AMPDU_METADATA_SIZE + 1] = 0x41;
        });
        owner
            .commit_ht(
                cookie,
                pool.claim_radio(index),
                ht_frame_request(0, 32, 8, 0, rate),
            )
            .unwrap();
    }

    let aggregate = owner.prepared_aggregate(cookie).unwrap();
    let config = HtAmpduTxConfig::new(rate, aggregate.bytes, aggregate.subframes).unwrap();
    let mut retry = AmpduRetryState::<4>::new(
        SequenceNumber::new(0).unwrap(),
        4,
        AmpduRetryPolicy {
            lifetime: oer_time::RadioDuration::from_micros(
                crate::tx::runtime::VENDOR_AMPDU_MSDU_LIFETIME_MICROS as u64,
            ),
            retain_single_mpdu: true,
        },
        radio_stamp(0),
    )
    .unwrap();
    let mut hardware = DetachingCompletionHardware::with_bitmap(0b0101);
    owner
        .submit(&mut hardware, cookie, LegacyTxQueue::BestEffort, config)
        .unwrap();
    let stale = oer_ieee80211_lower_mac::Ieee80211Stamp {
        generation: crate::clock_generation(2),
        ..radio_stamp(0)
    };
    assert!(matches!(
        owner.observe_retry_completion(&mut hardware, cookie, &mut retry, stale, true),
        Err(crate::tx::ampdu::RetainedAmpduRetryCompletionError::Retry(
            crate::tx::runtime::AmpduRetryError::ClockGenerationChanged { expected, observed }
        ))
     if expected == crate::clock_generation(1) && observed == crate::clock_generation(2)));
    assert!(
        hardware.completion.is_some(),
        "stale timing cannot consume the hardware completion"
    );
    assert_eq!(retry.current_subframes(), 4);
    assert_eq!(retry.aggregate_attempts(), 1);
    let observed = owner
        .observe_retry_completion(&mut hardware, cookie, &mut retry, radio_stamp(0), true)
        .unwrap()
        .unwrap();
    assert_eq!(observed.first_sequence, SequenceNumber::new(0).unwrap());
    assert_eq!(observed.subframes, 4);
    assert_eq!(
        observed.decision,
        AmpduRetryDecision::RetainAggregate { retry_mask: 0b1010 }
    );

    let retained = owner
        .retain_for_ampdu_retry(
            cookie,
            observed.decision.retry_mask(),
            AmpduRepublication::Retransmission,
        )
        .unwrap();
    assert_eq!(retained.subframes, 2);
    let retry_config = HtAmpduTxConfig::new(rate, retained.bytes, retained.subframes).unwrap();
    let mut hardware = DetachingCompletionHardware::successful();
    owner
        .submit(
            &mut hardware,
            cookie,
            LegacyTxQueue::BestEffort,
            retry_config,
        )
        .unwrap();
    let observed = owner
        .observe_retry_completion(&mut hardware, cookie, &mut retry, radio_stamp(0), true)
        .unwrap()
        .unwrap();
    assert_eq!(
        observed.decision,
        AmpduRetryDecision::Finish { retry_mask: 0 }
    );

    let (first, _) = owner.completed_frame(cookie, 0).unwrap();
    assert_eq!(first[0], 1);
    assert_eq!(first[1], 0x49);
    let (second, _) = owner.completed_frame(cookie, 1).unwrap();
    assert_eq!(second[0], 3);
    assert_eq!(second[1], 0x49);

    // A following partial BlockAck is expressed in the compacted logical
    // index space. Retaining logical slot one must therefore keep original
    // backing three, not physical backing one.
    let retry = owner
        .retain_for_ampdu_retry(cookie, 0b10, AmpduRepublication::Retransmission)
        .unwrap();
    assert_eq!(retry.subframes, 1);
    let retry_config = HtAmpduTxConfig::new(rate, retry.bytes, retry.subframes).unwrap();
    let mut hardware = DetachingCompletionHardware::successful();
    owner
        .submit(
            &mut hardware,
            cookie,
            LegacyTxQueue::BestEffort,
            retry_config,
        )
        .unwrap();
    assert!(
        owner
            .acknowledge_completion(&mut hardware)
            .unwrap()
            .is_some()
    );
    owner.detach_completed(&mut hardware, cookie).unwrap();

    let (last, _) = owner.completed_frame(cookie, 0).unwrap();
    assert_eq!(last[0], 3);
    assert_eq!(last[1], 0x49);
    let work = owner.work();
    assert_eq!(work.publications, 3);
    assert_eq!(work.aifs_slots, 6);
    assert_eq!(work.backoff_slots, 0);
    assert_eq!(work.unreported_contention, 0);
    assert_eq!(work.mpdus, 4 + 2 + 1);
    assert_eq!(
        work.psdu_bytes,
        u32::from(aggregate.bytes) + u32::from(retained.bytes) + u32::from(retry.bytes)
    );
    assert!(work.psdu_bytes < 3 * u32::from(aggregate.bytes));
    owner.release_completed(cookie).unwrap();
    assert_eq!(owner.work(), work);
    assert_eq!(pool.claimed_slots(), 0);
    let next = owner.begin().unwrap();
    assert_eq!(owner.work(), Default::default());
    owner.cancel(next).unwrap();
}

#[test]
fn owned_dma_pool_builds_two_mpdu_length_without_publishing_hardware() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<4>::new());
    let pool = frame_pool::<256, 2>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();
    let rate = unlimited_ht_rate();

    owner
        .commit_ht(
            cookie,
            frame(&pool, 0, 100, |bytes| bytes.fill(0xa5)),
            ht_frame_request(0, 100, 8, 0, rate),
        )
        .unwrap();
    owner
        .commit_ht(
            cookie,
            frame(&pool, 1, 101, |bytes| bytes.fill(0x5a)),
            ht_frame_request(0, 101, 8, 0, rate),
        )
        .unwrap();

    assert_eq!(
        owner.prepared_aggregate(cookie).unwrap(),
        HtAmpduLength {
            // First PSDU: delimiter 4 + length 112. Final PSDU:
            // delimiter 4 + length 113, with its three padding bytes
            // removed by the recovered tail rule.
            bytes: 233,
            subframes: 2,
        }
    );
    assert_eq!(owner.frame_count(), 2);
    assert_eq!(owner.state(), TxSlotState::Reserved);
    owner.cancel(cookie).unwrap();
    assert_eq!(owner.state(), TxSlotState::Free);
}

#[test]
fn referenced_commit_uses_the_retained_allocation_without_copying_payload() {
    let storage = HtAmpduTxStorage::<2>::new();
    let mut external = [0xa5_u8; 256];
    external[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 100].fill(0x5a);
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    storage
        .as_mut()
        .commit_referenced_frame(cookie, &mut external, frame_layout(0, 100, 8), 0)
        .unwrap();

    assert_eq!(
        storage.prepared_aggregate(cookie).unwrap(),
        HtAmpduLength {
            bytes: 116,
            subframes: 1,
        }
    );
    storage.as_mut().cancel(cookie).unwrap();
    assert_eq!(
        storage.buffer_addresses[0],
        external.as_ptr().addr(),
        "descriptor backing must remain the referenced allocation"
    );

    assert_eq!(u32::from_le_bytes(external[..4].try_into().unwrap()), 112);
    assert_eq!(
        &external[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 100],
        &[0x5a; 100]
    );
    // No staging buffer: each slot adds only its metadata to the storage,
    // fewer bytes than the 100-byte payload it describes, so the retained
    // lease holds the only copy of the frame.
    let per_slot = (core::mem::size_of::<HtAmpduTxStorage<4>>()
        - core::mem::size_of::<HtAmpduTxStorage<2>>())
        / 2;
    assert!(per_slot < 100, "{per_slot} bytes per slot");
}

#[test]
fn referenced_he_commit_uses_external_capacity_with_descriptor_only_storage() {
    let storage = HtAmpduTxStorage::<2>::new();
    let mut first = [0xa5_u8; 256];
    let mut second = [0x5a_u8; 256];
    let rate = HeRate::bcc_dcm(
        crate::tx::HeBccDcmMcs::Mcs3,
        crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
    );
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    for external in [&mut first, &mut second] {
        storage
            .as_mut()
            .commit_referenced_he_frame(
                cookie,
                external,
                HeAmpduFrameRequest::new(
                    frame_layout(0, 16, 8),
                    HeAmpduPolicy::new(
                        rate,
                        HtAmpduDensity::SixteenMicroseconds,
                        HeEdcaTxopLimit::DEFAULT,
                    ),
                ),
            )
            .unwrap();
    }

    assert_eq!(storage.empty_delimiters[..2], [1, 1]);
    assert_eq!(
        storage.prepared_aggregate(cookie).unwrap(),
        HtAmpduLength {
            bytes: 68,
            subframes: 2,
        }
    );
    assert_eq!(first[4], 1);
    assert_eq!(second[4], 1);
    assert_eq!(storage.buffer_addresses[0], first.as_ptr().addr());
    assert_eq!(storage.buffer_addresses[1], second.as_ptr().addr());
    storage.as_mut().cancel(cookie).unwrap();
}

#[test]
fn referenced_ht_commit_stops_at_the_vendor_rate_byte_ceiling() {
    let storage = HtAmpduTxStorage::<8>::new();
    let mut external = [[0xa5_u8; 1_600]; 8];
    let mcs0_sgi = HtRate::new(
        crate::tx::HtMcs::Mcs0,
        crate::tx::HtGuardInterval::Short400Ns,
        crate::tx::HtChannelWidth::Mhz40,
    );
    let mut storage = core::pin::pin!(storage);
    storage
        .as_mut()
        .configure_max_aggregate_bytes(u16::MAX)
        .unwrap();
    let cookie = storage.as_mut().begin().unwrap();

    for frame in external.iter_mut().take(6) {
        storage
            .as_mut()
            .commit_referenced_ht_frame(cookie, frame, ht_frame_request(0, 1_500, 8, 0, mcs0_sgi))
            .unwrap();
    }
    assert_eq!(
        storage.prepared_aggregate(cookie).unwrap(),
        HtAmpduLength {
            bytes: 9_096,
            subframes: 6,
        }
    );
    assert!(
        !storage
            .can_commit_referenced_ht_frame(cookie, 1_500, 8, 0, mcs0_sgi, external[6].len())
            .unwrap()
    );
    assert_eq!(
        storage.as_mut().commit_referenced_ht_frame(
            cookie,
            &mut external[6],
            ht_frame_request(0, 1_500, 8, 0, mcs0_sgi),
        ),
        Err(HtAmpduTxError::AggregateFull)
    );
    assert_eq!(storage.frame_count(), 6);
    storage.as_mut().cancel(cookie).unwrap();

    // The complete oracle table uses zero for SGI MCS7. That means this
    // particular leaf adds no ceiling: the peer/static limit still
    // permits a seventh MPDU.
    let mcs7_sgi = HtRate::new(
        crate::tx::HtMcs::Mcs7,
        crate::tx::HtGuardInterval::Short400Ns,
        crate::tx::HtChannelWidth::Mhz40,
    );
    let cookie = storage.as_mut().begin().unwrap();
    for frame in external.iter_mut().take(7) {
        storage
            .as_mut()
            .commit_referenced_ht_frame(cookie, frame, ht_frame_request(0, 1_500, 8, 0, mcs7_sgi))
            .unwrap();
    }
    assert_eq!(storage.frame_count(), 7);
    storage.as_mut().cancel(cookie).unwrap();
}

#[test]
fn owned_dma_pool_preserves_one_subframe_he_ampdu_length() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<2>::new());
    let pool = frame_pool::<256, 1>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();
    owner
        .commit_ht(
            cookie,
            frame(&pool, 0, 100, |bytes| bytes.fill(0xa5)),
            ht_frame_request(0, 100, 8, 0, unlimited_ht_rate()),
        )
        .unwrap();

    assert_eq!(
        owner.prepared_aggregate(cookie).unwrap(),
        HtAmpduLength {
            // One delimiter plus MPDU, hardware MIC and FCS. The 112-byte
            // PSDU is already aligned, so the tail rule removes nothing.
            bytes: 116,
            subframes: 1,
        }
    );
    owner.cancel(cookie).unwrap();
}

#[test]
fn owned_he_commit_derives_empty_delimiters_from_rate_and_peer_density() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<2>::new());
    let pool = frame_pool::<256, 2>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();
    let rate = HeRate::bcc_dcm(
        crate::tx::HeBccDcmMcs::Mcs3,
        crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
    );

    for (slot, payload) in [(0, 0xa5), (1, 0x5a)] {
        owner
            .commit_he(
                cookie,
                frame(&pool, slot, 16, |bytes| bytes.fill(payload)),
                he_frame_request(16, rate, HtAmpduDensity::SixteenMicroseconds),
            )
            .unwrap();
    }

    // frame + MIC + FCS is 28 bytes. At DCM MCS3/GI800 and 16 us the
    // blob minimum is 35 bytes, so ppCalDeliNum requests one empty
    // delimiter. The trailing delimiter of the final MPDU is omitted
    // from aggregate length exactly as ppEmptyDelimiterLength requires.
    assert_eq!(owner.empty_delimiters[..2], [1, 1]);
    assert_eq!(
        owner.prepared_aggregate(cookie).unwrap(),
        HtAmpduLength {
            bytes: 68,
            subframes: 2,
        }
    );
    owner.cancel(cookie).unwrap();
    let first = released_slot(&pool, 0, TX_AMPDU_METADATA_SIZE);
    assert_eq!(first[4], 1);
}

#[test]
fn he_trigger_preparation_fails_closed_without_original_msdu_length() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<2>::new());
    let pool = frame_pool::<256, 1>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();
    let rate = HeRate::new(
        crate::tx::HeMcs::Mcs0,
        crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
    );
    owner
        .commit_he(
            cookie,
            frame(&pool, 0, 64, |bytes| bytes.fill(0x5a)),
            he_frame_request(64, rate, HtAmpduDensity::NoRestriction),
        )
        .unwrap();
    let aggregate = owner.prepared_aggregate(cookie).unwrap();
    let trigger =
        crate::tx::HeTriggerBasedTxConfig::new(MacHeTbTidLimit::Three, MacHeTid::new(0).unwrap())
            .unwrap();
    let config = HeAmpduTxConfig::new(
        rate,
        0,
        aggregate.bytes,
        aggregate.subframes,
        HtAmpduDensity::NoRestriction,
    )
    .unwrap()
    .with_trigger_based(trigger);

    assert_eq!(
        owner.prepared_he_trigger(LegacyTxQueue::BestEffort, config),
        Err(HtAmpduTxError::TriggerMsduLengthUnavailable)
    );
    owner.cancel(cookie).unwrap();
}

#[test]
fn completion_exposes_publication_snapshot_then_clears_tb_enable() {
    let storage = HtAmpduTxStorage::<2>::new();
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    let reservation = MacHeTbLinkReservation::for_queue(MacHeTbTidLimit::Three, 2, 2).unwrap();
    let snapshot = MacHeTriggerTxQueueSnapshot {
        logical_queue: 2,
        tid: 0,
        trigger_based_enabled: true,
        mu_edca_timer_select: 2,
        mu_edca_timer_enabled: true,
        first_link: 0,
        first_mpdu_length: 112,
        first_next_link: 1,
        tail_link: 1,
        programmed_msdu_bytes: 161,
        queued_msdu_bytes: 161,
        queue_valid: true,
    };
    {
        let storage = storage.as_mut().project();
        *storage.state = TxSlotState::HardwareOwned;
        *storage.queue = LegacyTxQueue::BestEffort;
        *storage.trigger_reservation = Some(reservation);
        *storage.trigger_publication_snapshot = Some(snapshot);
    }
    let mut hardware = CompletionHardware {
        completion: Some(MacHtAmpduCompletionObservation::new_model(
            MacTxCompletionObservation::new_model(0, 0),
            0,
            0,
            0,
            true,
        )),
        cleared: None,
        trigger_snapshot: Some(snapshot),
    };

    assert_eq!(
        storage
            .as_ref()
            .he_trigger_based_snapshot(&hardware, cookie),
        Ok(Some(snapshot))
    );
    assert!(
        storage
            .as_mut()
            .acknowledge_completion(&mut hardware)
            .unwrap()
            .is_some()
    );
    assert_eq!(hardware.cleared, Some(reservation));
    assert_eq!(
        storage
            .as_ref()
            .he_trigger_based_snapshot(&hardware, cookie),
        Err(HtAmpduTxError::Stale)
    );
}

#[test]
fn incremental_pool_length_matches_blob_accumulator_for_full_window() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<32>::new());
    let pool = frame_pool::<256, 32>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    owner.configure_max_aggregate_bytes(u16::MAX).unwrap();
    let cookie = owner.begin().unwrap();
    let rate = unlimited_ht_rate();
    let mut oracle = HtAmpduLengthAccumulator::new(32, u16::MAX).unwrap();

    for index in 0..32_u8 {
        let frame_length = 100 + usize::from(index % 7);
        let empty_delimiters = index % 3;
        owner
            .commit_ht(
                cookie,
                frame(&pool, index, frame_length, |bytes| bytes.fill(index)),
                ht_frame_request(0, frame_length, 8, empty_delimiters, rate),
            )
            .unwrap();
        oracle
            .push(
                (frame_length + 8 + usize::from(TX_FCS_SIZE)) as u32,
                empty_delimiters,
            )
            .unwrap();
        if index != 0 {
            assert_eq!(
                owner.prepared_aggregate(cookie).unwrap(),
                oracle.finish().unwrap()
            );
        }
    }
    owner.cancel(cookie).unwrap();
}

#[test]
fn completed_pool_retains_mpdu_until_explicit_release() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<2>::new());
    let pool = frame_pool::<256, 1>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();
    let rate = unlimited_ht_rate();
    owner
        .commit_ht(
            cookie,
            frame(&pool, 0, 32, |bytes| bytes.fill(0x5a)),
            ht_frame_request(0, 32, 8, 0, rate),
        )
        .unwrap();
    let aggregate = owner.prepared_aggregate(cookie).unwrap();
    let mut hardware = DetachingCompletionHardware::successful();
    owner
        .submit(
            &mut hardware,
            cookie,
            LegacyTxQueue::BestEffort,
            HtAmpduTxConfig::new(rate, aggregate.bytes, aggregate.subframes).unwrap(),
        )
        .unwrap();

    // The two hardware ownership edges are independent: completion alone
    // must not expose a buffer that the queue still references.
    assert!(
        owner
            .acknowledge_completion(&mut hardware)
            .unwrap()
            .is_some()
    );
    assert_eq!(owner.state(), TxSlotState::Completed);
    assert_eq!(owner.completed_frame(cookie, 0), Err(HtAmpduTxError::Stale));
    owner.detach_completed(&mut hardware, cookie).unwrap();
    let (frame, mic_length) = owner.completed_frame(cookie, 0).unwrap();
    assert_eq!(frame, &[0x5a; 32]);
    assert_eq!(mic_length, 8);

    owner.release_completed(cookie).unwrap();
    assert_eq!(owner.state(), TxSlotState::Free);
    assert_eq!(owner.completed_frame(cookie, 0), Err(HtAmpduTxError::Stale));
}

#[test]
fn detached_pool_compacts_only_missing_frames_for_ampdu_retry() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<4>::new());
    let pool = frame_pool::<256, 4>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();
    let rate = unlimited_ht_rate();
    for index in 0..4_u8 {
        owner
            .commit_ht(
                cookie,
                frame(&pool, index, 32, |bytes| {
                    bytes.fill(index);
                    bytes[1] = 0x41;
                }),
                ht_frame_request(0, 32, 8, 0, rate),
            )
            .unwrap();
    }
    let committed = owner.buffer_addresses;
    complete_and_detach(&mut owner, cookie, rate);

    let aggregate = owner
        .retain_for_ampdu_retry(cookie, 0b1010, AmpduRepublication::Retransmission)
        .unwrap();
    assert_eq!(aggregate.subframes, 2);
    assert_eq!(owner.frame_count(), 2);
    assert_eq!(owner.state(), TxSlotState::Reserved);
    assert_eq!(owner.buffer_addresses[0], committed[1]);
    assert_eq!(owner.buffer_addresses[1], committed[3]);
    owner.cancel(cookie).unwrap();
    for slot in [1, 3] {
        let bytes = released_slot(&pool, slot, TX_AMPDU_METADATA_SIZE + 2);
        assert_eq!(bytes[TX_AMPDU_METADATA_SIZE], slot);
        assert_eq!(bytes[TX_AMPDU_METADATA_SIZE + 1], 0x49);
    }
}

#[test]
fn protection_failure_republishes_every_frame_without_the_retry_bit() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<4>::new());
    let pool = frame_pool::<256, 3>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();
    let rate = unlimited_ht_rate();
    for index in 0..3_u8 {
        owner
            .commit_ht(
                cookie,
                frame(&pool, index, 32, |bytes| {
                    bytes.fill(index);
                    bytes[1] = 0x41;
                }),
                ht_frame_request(0, 32, 8, 0, rate),
            )
            .unwrap();
    }
    let original = owner.prepared_aggregate(cookie).unwrap();
    let committed = owner.buffer_addresses;
    complete_and_detach(&mut owner, cookie, rate);

    assert_eq!(
        owner.retain_for_ampdu_retry(cookie, 0b011, AmpduRepublication::AfterProtectionFailure),
        Err(HtAmpduTxError::InvalidRetryMask {
            mask: 0b011,
            count: 3
        })
    );
    let aggregate = owner
        .retain_for_ampdu_retry(cookie, 0b111, AmpduRepublication::AfterProtectionFailure)
        .unwrap();
    assert_eq!(aggregate, original);
    assert_eq!(owner.state(), TxSlotState::Reserved);
    assert_eq!(owner.buffer_addresses[..3], committed[..3]);
    owner.cancel(cookie).unwrap();
    for slot in 0..3 {
        let bytes = released_slot(&pool, slot, TX_AMPDU_METADATA_SIZE + 2);
        assert_eq!(bytes[TX_AMPDU_METADATA_SIZE + 1], 0x41);
    }
}

#[test]
fn detached_he_pool_retains_one_missing_mpdu_at_the_original_rate() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<2>::new());
    let pool = frame_pool::<256, 1>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();
    let initial = HeRate::ldpc(
        crate::tx::HeMcs::Mcs9,
        crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns,
    );
    let density = HtAmpduDensity::EightMicroseconds;
    owner
        .commit_he(
            cookie,
            frame(&pool, 0, 32, |bytes| bytes.fill(0x5a)),
            he_frame_request(32, initial, density),
        )
        .unwrap();
    let aggregate = owner.prepared_aggregate(cookie).unwrap();
    let mut hardware = DetachingCompletionHardware::successful();
    owner
        .submit_he(
            &mut hardware,
            cookie,
            LegacyTxQueue::BestEffort,
            HeAmpduTxConfig::new(initial, 0, aggregate.bytes, aggregate.subframes, density)
                .unwrap(),
        )
        .unwrap();
    assert!(
        owner
            .acknowledge_completion(&mut hardware)
            .unwrap()
            .is_some()
    );
    owner.detach_completed(&mut hardware, cookie).unwrap();

    let retained = owner
        .retain_for_ampdu_retry(cookie, 0b1, AmpduRepublication::Retransmission)
        .unwrap();
    assert_eq!(retained.subframes, 1);
    assert_eq!(
        owner.prepared_empty_delimiters(cookie, 0).unwrap(),
        initial
            .ampdu_empty_delimiters(32 + 8 + TX_FCS_SIZE, density)
            .unwrap()
    );
    owner.cancel(cookie).unwrap();
    let buffer = released_slot(&pool, 0, TX_AMPDU_METADATA_SIZE + 2);
    assert_eq!(buffer[TX_AMPDU_METADATA_SIZE + 1] & 0x08, 0x08);
}

#[test]
fn full_window_and_byte_ceiling_are_independent() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<32>::new());
    let pool = frame_pool::<1700, 32>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let rate = unlimited_ht_rate();
    owner.configure_max_aggregate_bytes(0x7fff).unwrap();
    let cookie = owner.begin().unwrap();
    for slot in 0..20 {
        assert!(
            owner
                .can_commit_referenced_ht_frame(cookie, 1600, 8, 0, rate, 1700)
                .unwrap()
        );
        owner
            .commit_ht(
                cookie,
                frame(&pool, slot, 1600, |_| {}),
                ht_frame_request(0, 1600, 8, 0, rate),
            )
            .unwrap();
    }
    assert_eq!(owner.frame_count(), 20);
    assert!(
        !owner
            .can_commit_referenced_ht_frame(cookie, 1600, 8, 0, rate, 1700)
            .unwrap()
    );
    assert_eq!(
        owner.commit_ht(
            cookie,
            frame(&pool, 20, 1600, |_| {}),
            ht_frame_request(0, 1600, 8, 0, rate),
        ),
        Err(HtAmpduTxError::AggregateFull)
    );
    assert_eq!(owner.frame_count(), 20);
    owner.cancel(cookie).unwrap();

    owner.configure_max_aggregate_bytes(0x1fff).unwrap();
    let cookie = owner.begin().unwrap();
    for slot in 0..32 {
        assert!(
            owner
                .can_commit_referenced_ht_frame(cookie, 100, 8, 0, rate, 1700)
                .unwrap()
        );
        owner
            .commit_ht(
                cookie,
                frame(&pool, slot, 100, |_| {}),
                ht_frame_request(0, 100, 8, 0, rate),
            )
            .unwrap();
    }
    assert_eq!(owner.frame_count(), 32);
    assert!(
        !owner
            .can_commit_referenced_ht_frame(cookie, 100, 8, 0, rate, 1700)
            .unwrap()
    );
    owner.cancel(cookie).unwrap();
}

#[test]
fn he_rate_duration_gate_prevents_an_oversized_dma_publication() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<32>::new());
    let pool = frame_pool::<1_600, 32>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    owner.configure_max_aggregate_bytes(u16::MAX).unwrap();
    let density = HtAmpduDensity::NoRestriction;
    let gi_1600 = HeRate::ldpc(
        crate::tx::HeMcs::Mcs9,
        crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns,
    );
    let policy = |rate| HeAmpduPolicy::new(rate, density, HeEdcaTxopLimit::DEFAULT);
    let frame_size = AmpduFrameSize::new(1_500, 8);
    let cookie = owner.begin().unwrap();
    for slot in 0..31 {
        assert!(
            owner
                .can_commit_referenced_he_frame(cookie, frame_size, policy(gi_1600), 1_600)
                .unwrap()
        );
        owner
            .commit_he(
                cookie,
                frame(&pool, slot, 1_500, |_| {}),
                he_frame_request(1_500, gi_1600, density),
            )
            .unwrap();
    }
    assert_eq!(owner.prepared_aggregate(cookie).unwrap().bytes, 46_996);
    assert!(
        !owner
            .can_commit_referenced_he_frame(cookie, frame_size, policy(gi_1600), 1_600)
            .unwrap()
    );
    assert_eq!(
        owner.commit_he(
            cookie,
            frame(&pool, 31, 1_500, |_| {}),
            he_frame_request(1_500, gi_1600, density),
        ),
        Err(HtAmpduTxError::AggregateFull)
    );
    owner.cancel(cookie).unwrap();

    let gi_800 = HeRate::ldpc(
        crate::tx::HeMcs::Mcs9,
        crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
    );
    let cookie = owner.begin().unwrap();
    for slot in 0..32 {
        owner
            .commit_he(
                cookie,
                frame(&pool, slot, 1_500, |_| {}),
                he_frame_request(1_500, gi_800, density),
            )
            .unwrap();
    }
    assert_eq!(owner.prepared_aggregate(cookie).unwrap().bytes, 48_512);
    owner.cancel(cookie).unwrap();
}

#[test]
fn ht_ampdu_length_matches_the_s31_six_mpdu_oracle() {
    let mut length = HtAmpduLengthAccumulator::new(32, u16::MAX).unwrap();
    for sequence in 0x15_u32..=0x1a {
        // The hardware payload metadata was 0x00ss0612 followed by a zero
        // delimiter byte: six 1,554-byte MPDUs with two-byte padding.
        length.push((sequence << 16) | 0x0612, 0).unwrap();
    }
    assert_eq!(
        length.finish(),
        Ok(HtAmpduLength {
            bytes: 9_358,
            subframes: 6,
        })
    );
}

#[test]
fn ht_ampdu_length_is_bounded_and_removes_only_the_tail_trailer() {
    let mut length = HtAmpduLengthAccumulator::new(2, 4_096).unwrap();
    length.push(1_001, 2).unwrap();
    length.push(1_002, 1).unwrap();
    // First: 1001 + 3 padding + 8 empty + 4 mandatory.
    // Last: 1002 + 4 mandatory; its 2 padding + 4 empty bytes are removed.
    assert_eq!(length.finish().unwrap().bytes, 2_022);
    assert_eq!(length.push(1, 0), Err(HtAmpduLengthError::WindowFull));

    let mut too_short = HtAmpduLengthAccumulator::new(1, 1_000).unwrap();
    assert_eq!(
        too_short.push(1_001, 0),
        Err(HtAmpduLengthError::AggregateTooLong(1_005))
    );
    assert!(matches!(
        HtAmpduLengthAccumulator::new(0, 1),
        Err(HtAmpduLengthError::InvalidLimits)
    ));
}

#[test]
fn basic_ht_assembly_matches_the_s31_hardware_oracle() {
    assert_eq!(
        basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
            aggregate_length: 9_358,
            first_header_length: 34,
            first_payload_word: 0xff12_3456,
            first_descriptor_flags: 0x0004_2009,
            first_descriptor_word1: 0xa5a5_0020,
            first_rate: 33,
            tail_buffer_flags: 0xa186_8612,
            tail_timestamp: 0x1234_5678,
        }),
        Ok(BasicHtAmpduAssemblyOutput {
            first_remaining_length: 9_324,
            first_payload_word: 0xfe12_3456,
            first_descriptor_flags: 0x004c_2009,
            first_descriptor_word1: 0x20,
            tail_buffer_flags: 0xe186_8612,
            first_timestamp: 0x1234_5678,
        })
    );
}

#[test]
fn partial_block_ack_mutations_match_the_s31_retry_oracle() {
    let base = BasicHtAmpduCompletionInput {
        descriptor_flags: 0x0004_2009,
        descriptor_queue_word: 0x00a0_0304,
        frame_control: 0x4188,
        acknowledged: true,
    };
    assert_eq!(
        basic_ht_ampdu_completion(base),
        BasicHtAmpduCompletionOutput {
            descriptor_flags: 0x0044_2009,
            descriptor_queue_word: 0x01a0_0304,
            frame_control: 0x4188,
        }
    );
    assert_eq!(
        basic_ht_ampdu_completion(BasicHtAmpduCompletionInput {
            acknowledged: false,
            ..base
        }),
        BasicHtAmpduCompletionOutput {
            descriptor_flags: 0x0004_2009,
            descriptor_queue_word: 0x00a0_0304,
            frame_control: 0x4988,
        }
    );
}

#[test]
fn basic_ht_assembly_rejects_he_bar_ampdu_and_bad_lengths_before_mutation() {
    let input = BasicHtAmpduAssemblyInput {
        aggregate_length: 1_500,
        first_header_length: 34,
        first_payload_word: 0,
        first_descriptor_flags: 0x0004_2009,
        first_descriptor_word1: 0x20,
        first_rate: 33,
        tail_buffer_flags: 0xa186_8612,
        tail_timestamp: 0,
    };
    assert_eq!(
        basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
            first_descriptor_flags: input.first_descriptor_flags | TX_DESCRIPTOR_HE_BIT,
            ..input
        }),
        Err(BasicHtAmpduAssemblyError::UnsupportedDescriptor(
            0x8004_2009
        ))
    );
    assert_eq!(
        basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
            first_rate: 15,
            ..input
        }),
        Err(BasicHtAmpduAssemblyError::UnsupportedRate(15))
    );
    assert_eq!(
        basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
            aggregate_length: 33,
            ..input
        }),
        Err(BasicHtAmpduAssemblyError::AggregateShorterThanHeader)
    );
    assert_eq!(
        basic_ht_ampdu_assembly(BasicHtAmpduAssemblyInput {
            tail_buffer_flags: input.tail_buffer_flags | TX_BUFFER_END_BIT,
            ..input
        }),
        Err(BasicHtAmpduAssemblyError::TailAlreadyTerminated(
            0xe186_8612
        ))
    );
}

#[test]
fn block_ack_bitmap_handles_sequence_wrap() {
    let ack = TxBlockAckBitmap::new(SequenceNumber::new(0x0ffe).unwrap(), 0b1101);
    assert!(ack.acknowledges(SequenceNumber::new(0x0ffe).unwrap()));
    assert!(!ack.acknowledges(SequenceNumber::new(0x0fff).unwrap()));
    assert!(ack.acknowledges(SequenceNumber::new(0).unwrap()));
    assert!(ack.acknowledges(SequenceNumber::new(1).unwrap()));
    assert!(!ack.acknowledges(SequenceNumber::new(2).unwrap()));
}

#[test]
fn block_ack_bitmap_accepts_only_bounded_sequences_left_of_an_advanced_ssn() {
    let ack = TxBlockAckBitmap::new(SequenceNumber::new(105).unwrap(), 0b1);
    assert!(ack.acknowledges(SequenceNumber::new(105).unwrap()));
    assert!(ack.acknowledges(SequenceNumber::new(104).unwrap()));
    assert!(ack.acknowledges(SequenceNumber::new(100).unwrap()));
    assert!(!ack.acknowledges(SequenceNumber::new(106).unwrap()));
    assert!(!ack.acknowledges(SequenceNumber::new(40).unwrap()));

    let wrapped = TxBlockAckBitmap::new(SequenceNumber::new(0).unwrap(), 0b1);
    assert!(wrapped.acknowledges(SequenceNumber::new(0x0fff).unwrap()));
    assert!(!wrapped.acknowledges(SequenceNumber::new(65).unwrap()));
}

#[test]
fn batch_returns_one_block_ack_result_per_step() {
    let mut batch = TxAmpduBatch::new();
    batch
        .begin(SequenceNumber::new(0x0ffe).unwrap(), 4)
        .unwrap();
    for slot in 3..7 {
        batch.push(slot).unwrap();
    }
    batch
        .complete_with_block_ack(TxBlockAckBitmap::new(
            SequenceNumber::new(0x0ffe).unwrap(),
            0b1101,
        ))
        .unwrap();

    for (slot, sequence, disposition) in [
        (3, 0x0ffe, TxAmpduDisposition::Acknowledged),
        (4, 0x0fff, TxAmpduDisposition::Retry),
        (5, 0, TxAmpduDisposition::Acknowledged),
        (6, 1, TxAmpduDisposition::Acknowledged),
    ] {
        assert_eq!(
            batch.next_completion(),
            Some(TxAmpduCompletion {
                mpdu: TxAmpduMpdu {
                    slot: TxAmpduSlot::new(slot).unwrap(),
                    sequence: SequenceNumber::new(sequence).unwrap(),
                },
                disposition,
            })
        );
    }
    assert!(batch.is_idle());
    assert_eq!(batch.next_completion(), None);
}

#[test]
fn missing_block_ack_retries_every_mpdu_without_a_drain() {
    let mut batch = TxAmpduBatch::new();
    batch.begin(SequenceNumber::new(9).unwrap(), 2).unwrap();
    batch.push(0).unwrap();
    batch.push(31).unwrap();
    batch.complete_without_block_ack().unwrap();
    assert_eq!(
        batch.next_completion().unwrap().disposition,
        TxAmpduDisposition::Retry
    );
    assert!(!batch.is_idle());
    assert_eq!(
        batch.next_completion().unwrap().disposition,
        TxAmpduDisposition::Retry
    );
    assert!(batch.is_idle());
}

#[test]
fn batch_rejects_duplicate_static_slot_ownership() {
    let mut batch = TxAmpduBatch::new();
    batch.begin(SequenceNumber::new(0).unwrap(), 32).unwrap();
    batch.push(17).unwrap();
    assert_eq!(batch.push(17), Err(TxAmpduBatchError::DuplicateSlot(17)));
}

#[test]
fn batch_preserves_nonconsecutive_hardware_sequences() {
    let mut batch = TxAmpduBatch::new();
    batch.begin(SequenceNumber::new(0x120).unwrap(), 4).unwrap();
    assert_eq!(
        batch
            .push_sequence(3, SequenceNumber::new(0x120).unwrap())
            .unwrap()
            .sequence,
        SequenceNumber::new(0x120).unwrap()
    );
    assert_eq!(
        batch
            .push_sequence(4, SequenceNumber::new(0x123).unwrap())
            .unwrap()
            .sequence,
        SequenceNumber::new(0x123).unwrap()
    );
    assert_eq!(
        batch.push_sequence(5, SequenceNumber::new(0x123).unwrap()),
        Err(TxAmpduBatchError::DuplicateSequence(
            SequenceNumber::new(0x123).unwrap()
        ))
    );
    batch
        .complete_with_block_ack(TxBlockAckBitmap::new(
            SequenceNumber::new(0x120).unwrap(),
            0b1001,
        ))
        .unwrap();
    assert_eq!(
        batch.next_completion().unwrap().disposition,
        TxAmpduDisposition::Acknowledged
    );
    assert_eq!(
        batch.next_completion().unwrap().disposition,
        TxAmpduDisposition::Acknowledged
    );
    assert!(batch.is_idle());
}

#[test]
fn batch_never_exceeds_negotiated_or_static_window() {
    let mut batch = TxAmpduBatch::new();
    assert_eq!(
        batch.begin(SequenceNumber::new(0).unwrap(), 0),
        Err(TxAmpduBatchError::InvalidWindow(0))
    );
    assert_eq!(
        batch.begin(SequenceNumber::new(0).unwrap(), 33),
        Err(TxAmpduBatchError::InvalidWindow(33))
    );
    batch.begin(SequenceNumber::new(0).unwrap(), 1).unwrap();
    batch.push(0).unwrap();
    assert_eq!(batch.push(1), Err(TxAmpduBatchError::Full));
}

mod work;

mod exchange_budget;

fn radio_stamp(micros: u64) -> oer_ieee80211_lower_mac::Ieee80211Stamp {
    oer_ieee80211_lower_mac::Ieee80211Stamp {
        at: oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(micros),
        generation: crate::clock_generation(1),
    }
}

#[test]
fn no_effect_keeps_the_aggregate_and_its_trigger_reservation_published() {
    use crate::tx::CompletionObservation;
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<2>::new());
    let cookie = storage.as_mut().begin().unwrap();
    let reservation = MacHeTbLinkReservation::for_queue(MacHeTbTidLimit::Three, 2, 2).unwrap();
    {
        let storage = storage.as_mut().project();
        *storage.state = TxSlotState::HardwareOwned;
        *storage.queue = LegacyTxQueue::BestEffort;
        *storage.trigger_reservation = Some(reservation);
    }
    let mut hardware = CompletionHardware {
        completion: Some(MacHtAmpduCompletionObservation::new_model(
            MacTxCompletionObservation::new_model(1, 2),
            0,
            0,
            0,
            false,
        )),
        cleared: None,
        trigger_snapshot: None,
    };
    assert_eq!(
        storage.as_mut().acknowledge_completion(&mut hardware),
        Ok(Some(CompletionObservation::NoEffect))
    );
    assert_eq!(storage.state(), TxSlotState::HardwareOwned);
    assert_eq!(hardware.cleared, None);
    assert_eq!(
        storage.as_mut().project().trigger_reservation,
        &Some(reservation)
    );
    hardware.completion = Some(MacHtAmpduCompletionObservation::new_model(
        MacTxCompletionObservation::new_model(0, 0),
        0,
        0,
        1,
        true,
    ));
    assert!(matches!(
        storage.as_mut().acknowledge_completion(&mut hardware),
        Ok(Some(CompletionObservation::Observed(_)))
    ));
    assert_eq!(storage.state(), TxSlotState::Completed);
    assert_eq!(hardware.cleared, Some(reservation));
    assert_eq!(cookie, storage.active);
}
