//! Host/oracle tests for A-MPDU protocol, formatting and ownership.

use oer_chips_esp32s31_hal::layouts::{DmaRef, WifiTxDmaDescriptor};

use super::*;
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

use oer_chips_esp32s31_hal::memory::dma_static::{Placed, PlacedTxLease, PlacedTxPool};

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
    PlacedTxPool::pin(Placed::host(storage)).unwrap_or_else(|_| panic!("placed pool geometry"))
}
mod length_budget;
use crate::mac::tx::{
    HeAmpduTxConfig, HeRate, HtAmpduDensity, HtAmpduTxConfig, HtRate, TxHardware,
};
use oer_chips_esp32s31_hal::types::{
    MacHeTbTidLimit, MacHeTid, MacHtAmpduCompletionObservation, MacHtTxProgram, MacLegacyTxProgram,
    MacTxAbortCause, MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason,
    MacTxQueueDetached,
};
use oer_portable_memory::{
    HardwareOwnedTxDma, PinnedDmaTxPool, PinnedDmaTxRadioLease, PreparedTxDma,
};

struct CompletionHardware {
    completion: Option<MacHtAmpduCompletionObservation>,
    cleared: Option<MacHeTbLinkReservation>,
    trigger_snapshot: Option<MacHeTriggerTxQueueSnapshot>,
}

impl TxHardware for CompletionHardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        _: MacLegacyTxProgram,
    ) -> bool {
        false
    }

    fn start_bound_legacy_tx(
        &mut self,
        _: &impl HardwareOwnedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
    ) {
    }

    fn prepare_bound_ht_tx(
        &mut self,
        _: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        _: MacHtTxProgram,
    ) -> bool {
        true
    }

    fn start_bound_ht_tx(
        &mut self,
        _: &impl HardwareOwnedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
    ) {
    }

    fn take_tx_completion(
        &mut self,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
    ) -> Option<MacTxCompletionObservation> {
        None
    }

    fn begin_tx_timeout_abort(
        &mut self,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        _: MacTxAbortCause,
    ) -> bool {
        false
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        _: u32,
        _: MacTxDetachReason,
        _: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        MacTxDetachOutcome::NoEvent
    }

    fn take_block_ack_completion(
        &mut self,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
    ) -> Option<MacHtAmpduCompletionObservation> {
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
    control: Option<oer_chips_esp32s31_hal::types::MacTxControlFrame>,
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
        _: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        _: MacLegacyTxProgram,
    ) -> bool {
        false
    }

    fn start_bound_legacy_tx(
        &mut self,
        _: &impl HardwareOwnedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
    ) {
    }

    fn prepare_bound_ht_tx(
        &mut self,
        _: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        program: MacHtTxProgram,
    ) -> bool {
        if !self.reject_publication {
            self.control = Some(program.control());
        }
        !self.reject_publication
    }

    fn start_bound_ht_tx(
        &mut self,
        _: &impl HardwareOwnedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
    ) {
    }

    fn prepare_bound_he_tx(
        &mut self,
        _: &impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        program: oer_chips_esp32s31_hal::types::MacHeTxProgram,
    ) -> bool {
        if !self.reject_publication {
            self.control = Some(program.control());
        }
        !self.reject_publication
    }

    fn take_tx_completion(
        &mut self,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
    ) -> Option<MacTxCompletionObservation> {
        None
    }

    fn begin_tx_timeout_abort(
        &mut self,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        _: MacTxAbortCause,
    ) -> bool {
        self.abort_requested = true;
        true
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
        descriptor_head: u32,
        _: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        if self.detach_failed {
            return MacTxDetachOutcome::Failed;
        }
        MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(descriptor_head)))
    }

    fn take_block_ack_completion(
        &mut self,
        _: oer_chips_esp32s31_hal::types::MacTxQueueIndex,
    ) -> Option<MacHtAmpduCompletionObservation> {
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

/// Take `backing`, whose MPDU of `length` octets is encoded behind the
/// metadata prefix, as the batch's next frame, as an arena's intake does:
/// its Frame Control becomes a Data frame's whose Protected bit asks for a
/// hardware MIC of `mic` octets; the frame's other octets stay.
fn commit<B: oer_chips_esp32s31_hal::memory::dma_static::PlacedBacking, const SLOTS: usize>(
    owner: &mut RetainedDmaAmpduTx<'_, B, SLOTS>,
    cookie: TxCookie,
    backing: B,
    length: usize,
    mic: u8,
) -> Result<(), HtAmpduTxError> {
    owner
        .intake(cookie, backing, |frame| {
            let [control, flags, ..] = frame else {
                return None;
            };
            *control = 0x08;
            *flags = if mic == 0 {
                *flags & !0x40
            } else {
                *flags | 0x40
            };
            u16::try_from(length).ok()
        })
        .map_err(|(_, refused)| match refused {
            AmpduIntakeRefused::Length => HtAmpduTxError::FrameTooLong,
            AmpduIntakeRefused::Full => HtAmpduTxError::AggregateFull,
            AmpduIntakeRefused::Declined => HtAmpduTxError::Stale,
            AmpduIntakeRefused::Fault(error) => error,
        })
}

/// The reserved batch's length as an HT publication prepares it.
fn prepared<B: oer_chips_esp32s31_hal::memory::dma_static::PlacedBacking, const SLOTS: usize>(
    owner: &mut RetainedDmaAmpduTx<'_, B, SLOTS>,
    cookie: TxCookie,
) -> Result<HtAmpduLength, HtAmpduTxError> {
    owner.prepare_format(cookie, |_| Some(0), false)
}

/// The batch's length as an HE publication at `rate` and the recipient's
/// `density` prepares it: each frame's empty delimiters from its PSDU.
fn prepared_he<B: oer_chips_esp32s31_hal::memory::dma_static::PlacedBacking, const SLOTS: usize>(
    owner: &mut RetainedDmaAmpduTx<'_, B, SLOTS>,
    cookie: TxCookie,
    rate: HeRate,
    density: HtAmpduDensity,
) -> Result<HtAmpduLength, HtAmpduTxError> {
    owner.prepare_format(
        cookie,
        |psdu| rate.ampdu_empty_delimiters(psdu, density),
        false,
    )
}

/// Record an MPDU encoded in `external` as the storage's next frame.
fn commit_referenced<const SLOTS: usize>(
    storage: core::pin::Pin<&mut HtAmpduTxStorage<SLOTS>>,
    cookie: TxCookie,
    external: &mut [u8],
    layout: AmpduFrameLayout,
    empty_delimiters: u8,
) -> Result<(), HtAmpduTxError> {
    let frame = storage.as_ref().get_ref().prepare_referenced_frame(
        cookie,
        external,
        layout,
        empty_delimiters,
    )?;
    storage.record_referenced_frame(cookie, frame)
}

/// An HT rate whose vendor A-MPDU byte table adds no ceiling, so only the
/// slot, backing and peer limits admit a frame.
fn unlimited_ht_rate() -> HtRate {
    let rate = HtRate::new(
        crate::mac::tx::HtMcs::Mcs7,
        crate::mac::tx::HtGuardInterval::Short400Ns,
        crate::mac::tx::HtChannelWidth::Mhz40,
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
    let (index, ()) = pool.claim_network(slot).unwrap().publish(
        oer_portable_memory::FrameLen::new(TX_AMPDU_METADATA_SIZE + length)
            .expect("test frame length"),
        |bytes| {
            encode(&mut bytes[TX_AMPDU_METADATA_SIZE..]);
        },
    );
    pool.claim_radio(index).expect("ready placed slot")
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
        .unwrap()
        .try_publish(
            oer_portable_memory::FrameLen::new(length).expect("test frame length"),
            |bytes| Err::<(), _>(bytes.to_vec()),
        )
        .err()
        .unwrap();
    lease.release();
    bytes
}

/// Publish the committed HT aggregate, then complete and detach it the way
/// the MAC reports a finished exchange.
fn complete_and_detach<
    B: oer_chips_esp32s31_hal::memory::dma_static::PlacedBacking,
    const SLOTS: usize,
>(
    owner: &mut RetainedDmaAmpduTx<'_, B, SLOTS>,
    cookie: TxCookie,
    rate: HtRate,
) {
    let aggregate = prepared(owner, cookie).unwrap();
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
    let network = pool.claim_network(0).unwrap();
    let (index, ()) = network.publish(
        oer_portable_memory::FrameLen::new(TX_AMPDU_METADATA_SIZE + 32).expect("test frame length"),
        |bytes| {
            bytes[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 32].fill(0x5a);
        },
    );
    let backing = pool.claim_radio(index).expect("ready placed slot");
    let mut retention = RetainedAmpduDmaStorage::new();

    {
        let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
        let cookie = owner.begin().unwrap();
        commit(&mut owner, cookie, backing, 32, 8).unwrap();
        assert_eq!(owner.held_backing_count(), 1);
    }

    assert_eq!(storage.state(), TxDmaState::Free);
    assert_eq!(pool.claim_network(0).unwrap().release(), 0);
}

#[test]
fn rejected_referenced_commit_rolls_back_the_lower_lease() {
    let storage = HtAmpduTxStorage::<2>::new();
    let mut storage = core::pin::pin!(storage);
    let pool: PlacedTxPool<256, 0, 0, 1> = host_pool(std::boxed::Box::leak(std::boxed::Box::new(
        PinnedDmaTxPool::new(),
    )));
    let network = pool.claim_network(0).unwrap();
    let (index, ()) = network.publish(
        oer_portable_memory::FrameLen::new(TX_AMPDU_METADATA_SIZE + 32).expect("test frame length"),
        |_| {},
    );
    let backing = pool.claim_radio(index).expect("ready placed slot");

    {
        let mut retention = RetainedAmpduDmaStorage::new();
        let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
        let cookie = owner.begin().unwrap();
        // Longer than the lease's region past its prefix and trailer.
        assert_eq!(
            commit(&mut owner, cookie, backing, 256, 8),
            Err(HtAmpduTxError::FrameTooLong)
        );
        assert_eq!(owner.held_backing_count(), 0);
        owner.cancel(cookie, drop).unwrap();
    }

    assert_eq!(pool.claim_network(0).unwrap().release(), 0);
}

#[test]
fn retained_dma_owner_quarantines_hardware_owned_backing_without_drop_panic() {
    let storage = HtAmpduTxStorage::<2>::new();
    let mut storage = core::pin::pin!(storage);
    let pool: PlacedTxPool<256, 0, 0, 1> = host_pool(std::boxed::Box::leak(std::boxed::Box::new(
        PinnedDmaTxPool::new(),
    )));
    let network = pool.claim_network(0).unwrap();
    let (index, ()) = network.publish(
        oer_portable_memory::FrameLen::new(TX_AMPDU_METADATA_SIZE + 32).expect("test frame length"),
        |bytes| {
            bytes[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 32].fill(0x5a);
        },
    );
    let backing = pool.claim_radio(index).expect("ready placed slot");

    {
        let mut retention = RetainedAmpduDmaStorage::new();
        let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
        let cookie = owner.begin().unwrap();
        let rate = HtRate::new(
            crate::mac::tx::HtMcs::Mcs0,
            crate::mac::tx::HtGuardInterval::Long800Ns,
            crate::mac::tx::HtChannelWidth::Mhz20,
        );
        commit(&mut owner, cookie, backing, 32, 8).unwrap();
        let aggregate = prepared(&mut owner, cookie).unwrap();
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

    assert_eq!(storage.state(), TxDmaState::ResetRequired);
    assert_eq!(pool.claimed_slots(), 1);
}

#[test]
fn retained_abort_releases_backing_only_after_successful_detach() {
    for detach_failed in [false, true] {
        let mut storage = core::pin::pin!(HtAmpduTxStorage::<2>::new());
        let pool: PlacedTxPool<256, 0, 0, 1> = host_pool(std::boxed::Box::leak(
            std::boxed::Box::new(PinnedDmaTxPool::new()),
        ));
        let (index, ()) = pool.claim_network(0).unwrap().publish(
            oer_portable_memory::FrameLen::new(TX_AMPDU_METADATA_SIZE + 32)
                .expect("test frame length"),
            |_| {},
        );
        let mut retention = RetainedAmpduDmaStorage::new();
        let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
        let cookie = owner.begin().unwrap();
        let rate = HtRate::new(
            crate::mac::tx::HtMcs::Mcs0,
            crate::mac::tx::HtGuardInterval::Long800Ns,
            crate::mac::tx::HtChannelWidth::Mhz20,
        );
        commit(
            &mut owner,
            cookie,
            pool.claim_radio(index).expect("ready placed slot"),
            32,
            8,
        )
        .unwrap();
        let aggregate = prepared(&mut owner, cookie).unwrap();
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

        assert!(
            owner
                .begin_timeout_abort(&mut hardware, cookie, MacTxAbortCause::TimeoutEdge)
                .unwrap()
        );
        assert!(hardware.abort_requested);
        assert_eq!(owner.held_backing_count(), 1);
        assert_eq!(pool.claimed_slots(), 1);
        // The caller retains the owner while waiting for the settle deadline.
        let result = owner.finish_timeout_abort(&mut hardware, cookie, drop);
        if detach_failed {
            assert_eq!(result, Err(HtAmpduTxError::DetachFailed));
            drop(owner);
            assert_eq!(storage.state(), TxDmaState::ResetRequired);
            assert_eq!(pool.claimed_slots(), 1);
        } else {
            result.unwrap();
            assert_eq!(owner.held_backing_count(), 0);
            assert_eq!(pool.claimed_slots(), 0);
            assert_eq!(
                owner.finish_timeout_abort(&mut hardware, cookie, drop),
                Err(HtAmpduTxError::Stale)
            );
            drop(owner);
            assert_eq!(storage.state(), TxDmaState::Free);
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
        crate::mac::tx::HtMcs::Mcs0,
        crate::mac::tx::HtGuardInterval::Long800Ns,
        crate::mac::tx::HtChannelWidth::Mhz20,
    );
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();

    for slot in 0..4 {
        let network = pool.claim_network(slot).unwrap();
        let (index, ()) = network.publish(
            oer_portable_memory::FrameLen::new(TX_AMPDU_METADATA_SIZE + 32)
                .expect("test frame length"),
            |bytes| {
                bytes[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 32].fill(slot);
                bytes[TX_AMPDU_METADATA_SIZE + 1] = 0x41;
            },
        );
        commit(
            &mut owner,
            cookie,
            pool.claim_radio(index).expect("ready placed slot"),
            32,
            8,
        )
        .unwrap();
    }

    let aggregate = prepared(&mut owner, cookie).unwrap();
    let config = HtAmpduTxConfig::new(rate, aggregate.bytes, aggregate.subframes).unwrap();
    // The BlockAck acknowledges frames 0 and 2: the caller keeps 1 and 3,
    // which went, so they go again with their Retry bit.
    let mut hardware = DetachingCompletionHardware::with_bitmap(0b0101);
    owner
        .submit(&mut hardware, cookie, LegacyTxQueue::BestEffort, config)
        .unwrap();
    assert!(
        owner
            .acknowledge_completion(&mut hardware)
            .unwrap()
            .is_some()
    );
    owner.detach_completed(&mut hardware, cookie).unwrap();
    let mut returned = 0;
    owner
        .retain_frames(cookie, 0b1010, 0b1010, |backing| {
            returned += 1;
            drop(backing);
        })
        .unwrap();
    assert_eq!(returned, 2, "the acknowledged frames return at once");
    assert_eq!(pool.claimed_slots(), 2);
    let retained = prepared(&mut owner, cookie).unwrap();
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
    assert!(
        owner
            .acknowledge_completion(&mut hardware)
            .unwrap()
            .is_some()
    );
    owner.detach_completed(&mut hardware, cookie).unwrap();
    let first = owner.observed_frame(0).unwrap();
    assert_eq!(first[2], 1);
    assert_eq!(first[1], 0x49);
    let second = owner.observed_frame(1).unwrap();
    assert_eq!(second[2], 3);
    assert_eq!(second[1], 0x49);

    // A following partial BlockAck is expressed in the compacted position
    // space. Keeping position one must therefore keep original backing
    // three, not physical backing one.
    owner.retain_frames(cookie, 0b10, 0b10, drop).unwrap();
    let retry = prepared(&mut owner, cookie).unwrap();
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

    let last = owner.observed_frame(0).unwrap();
    assert_eq!(last[2], 3);
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
    owner.release_completed(cookie, drop).unwrap();
    assert_eq!(owner.work(), work);
    assert_eq!(pool.claimed_slots(), 0);
    let next = owner.begin().unwrap();
    assert_eq!(owner.work(), Default::default());
    owner.cancel(next, drop).unwrap();
}

#[test]
fn owned_dma_pool_builds_two_mpdu_length_without_publishing_hardware() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<4>::new());
    let pool = frame_pool::<256, 2>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();

    commit(
        &mut owner,
        cookie,
        frame(&pool, 0, 100, |bytes| bytes.fill(0xa5)),
        100,
        8,
    )
    .unwrap();
    commit(
        &mut owner,
        cookie,
        frame(&pool, 1, 101, |bytes| bytes.fill(0x5a)),
        101,
        8,
    )
    .unwrap();

    assert_eq!(
        prepared(&mut owner, cookie).unwrap(),
        HtAmpduLength {
            // First PSDU: delimiter 4 + length 112. Final PSDU:
            // delimiter 4 + length 113, with its three padding bytes
            // removed by the recovered tail rule.
            bytes: 233,
            subframes: 2,
        }
    );
    assert_eq!(owner.frame_count(), 2);
    assert_eq!(owner.state(), TxDmaState::Reserved);
    owner.cancel(cookie, drop).unwrap();
    assert_eq!(owner.state(), TxDmaState::Free);
}

#[test]
fn referenced_commit_uses_the_retained_allocation_without_copying_payload() {
    let storage = HtAmpduTxStorage::<2>::new();
    let mut external = [0xa5_u8; 256];
    external[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + 100].fill(0x5a);
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    commit_referenced(
        storage.as_mut(),
        cookie,
        &mut external,
        frame_layout(0, 100, 8),
        0,
    )
    .unwrap();

    assert_eq!(
        storage.calculate_aggregate().unwrap(),
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
        crate::mac::tx::HeBccDcmMcs::Mcs3,
        crate::mac::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
    );
    let mut storage = core::pin::pin!(storage);
    let cookie = storage.as_mut().begin().unwrap();
    // frame + MIC + FCS is 28 octets; the rate and the recipient's
    // density give it one empty delimiter.
    let delimiters = rate
        .ampdu_empty_delimiters(16 + 8 + TX_FCS_SIZE, HtAmpduDensity::SixteenMicroseconds)
        .unwrap();
    for external in [&mut first, &mut second] {
        commit_referenced(
            storage.as_mut(),
            cookie,
            external,
            frame_layout(0, 16, 8),
            delimiters,
        )
        .unwrap();
    }

    assert_eq!(storage.empty_delimiters[..2], [1, 1]);
    assert_eq!(
        storage.calculate_aggregate().unwrap(),
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
fn owned_dma_pool_preserves_one_subframe_he_ampdu_length() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<2>::new());
    let pool = frame_pool::<256, 1>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();
    commit(
        &mut owner,
        cookie,
        frame(&pool, 0, 100, |bytes| bytes.fill(0xa5)),
        100,
        8,
    )
    .unwrap();

    assert_eq!(
        prepared(&mut owner, cookie).unwrap(),
        HtAmpduLength {
            // One delimiter plus MPDU, hardware MIC and FCS. The 112-byte
            // PSDU is already aligned, so the tail rule removes nothing.
            bytes: 116,
            subframes: 1,
        }
    );
    owner.cancel(cookie, drop).unwrap();
}

#[test]
fn an_he_format_derives_empty_delimiters_from_rate_and_peer_density() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<2>::new());
    let pool = frame_pool::<256, 2>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();
    let rate = HeRate::bcc_dcm(
        crate::mac::tx::HeBccDcmMcs::Mcs3,
        crate::mac::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
    );

    for (slot, payload) in [(0, 0xa5), (1, 0x5a)] {
        commit(
            &mut owner,
            cookie,
            frame(&pool, slot, 16, |bytes| bytes.fill(payload)),
            16,
            8,
        )
        .unwrap();
    }

    // frame + MIC + FCS is 28 bytes. At DCM MCS3/GI800 and 16 us the
    // blob minimum is 35 bytes, so ppCalDeliNum requests one empty
    // delimiter. The trailing delimiter of the final MPDU is omitted
    // from aggregate length exactly as ppEmptyDelimiterLength requires.
    assert_eq!(
        prepared_he(
            &mut owner,
            cookie,
            rate,
            HtAmpduDensity::SixteenMicroseconds
        )
        .unwrap(),
        HtAmpduLength {
            bytes: 68,
            subframes: 2,
        }
    );
    assert_eq!(owner.empty_delimiters[..2], [1, 1]);
    owner.cancel(cookie, drop).unwrap();
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
        crate::mac::tx::HeMcs::Mcs0,
        crate::mac::rx::HeGuardIntervalAndLtf::TwoLtf800Ns,
    );
    commit(
        &mut owner,
        cookie,
        frame(&pool, 0, 64, |bytes| bytes.fill(0x5a)),
        64,
        8,
    )
    .unwrap();
    let aggregate = prepared_he(&mut owner, cookie, rate, HtAmpduDensity::NoRestriction).unwrap();
    let trigger = crate::mac::tx::HeTriggerBasedTxConfig::new(
        MacHeTbTidLimit::Three,
        MacHeTid::new(0).unwrap(),
    )
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
    owner.cancel(cookie, drop).unwrap();
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
        *storage.state = TxDmaState::HardwareOwned;
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
    let mut oracle = HtAmpduLengthAccumulator::new(32, u16::MAX).unwrap();

    // Each PSDU's empty delimiters follow from its length, as an HE
    // publication's do.
    let delimiters = |psdu: u16| Some(u8::try_from(psdu % 3).unwrap());
    for index in 0..32_u8 {
        let frame_length = 100 + usize::from(index % 7);
        let psdu = (frame_length + 8 + usize::from(TX_FCS_SIZE)) as u16;
        commit(
            &mut owner,
            cookie,
            frame(&pool, index, frame_length, |bytes| bytes.fill(index)),
            frame_length,
            8,
        )
        .unwrap();
        oracle
            .push(u32::from(psdu), delimiters(psdu).unwrap())
            .unwrap();
        if index != 0 {
            assert_eq!(
                owner.prepare_format(cookie, delimiters, false).unwrap(),
                oracle.finish().unwrap()
            );
        }
    }
    owner.cancel(cookie, drop).unwrap();
}

#[test]
fn completed_pool_retains_mpdu_until_explicit_release() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<2>::new());
    let pool = frame_pool::<256, 1>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    let cookie = owner.begin().unwrap();
    let rate = unlimited_ht_rate();
    commit(
        &mut owner,
        cookie,
        frame(&pool, 0, 32, |bytes| bytes.fill(0x5a)),
        32,
        8,
    )
    .unwrap();
    let aggregate = prepared(&mut owner, cookie).unwrap();
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
    assert_eq!(owner.state(), TxDmaState::Completed);
    assert_eq!(pool.claimed_slots(), 1);
    // A retention needs the detach as well.
    assert_eq!(
        owner.retain_frames(cookie, 0b1, 0b1, drop),
        Err(HtAmpduTxError::Stale)
    );
    owner.detach_completed(&mut hardware, cookie).unwrap();
    let frame = owner.observed_frame(0).unwrap();
    assert_eq!(frame.len(), 32);
    assert_eq!(&frame[2..], &[0x5a; 30]);
    assert_eq!(owner.frame_size(0), Some((32, 8)));
    assert_eq!(pool.claimed_slots(), 1, "the detached frame stays retained");

    owner.release_completed(cookie, drop).unwrap();
    assert_eq!(owner.state(), TxDmaState::Free);
    assert_eq!(pool.claimed_slots(), 0);
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
        commit(
            &mut owner,
            cookie,
            frame(&pool, index, 32, |bytes| {
                bytes.fill(index);
                bytes[1] = 0x41;
            }),
            32,
            8,
        )
        .unwrap();
    }
    let committed = owner.buffer_addresses;
    complete_and_detach(&mut owner, cookie, rate);

    owner.retain_frames(cookie, 0b1010, 0b1010, drop).unwrap();
    assert_eq!(prepared(&mut owner, cookie).unwrap().subframes, 2);
    assert_eq!(owner.frame_count(), 2);
    assert_eq!(pool.claimed_slots(), 2, "the acknowledged frames returned");
    assert_eq!(owner.state(), TxDmaState::Reserved);
    assert_eq!(owner.buffer_addresses[0], committed[1]);
    assert_eq!(owner.buffer_addresses[1], committed[3]);
    owner.cancel(cookie, drop).unwrap();
    for slot in [1, 3] {
        let bytes = released_slot(&pool, slot, TX_AMPDU_METADATA_SIZE + 3);
        assert_eq!(bytes[TX_AMPDU_METADATA_SIZE + 2], slot);
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
        commit(
            &mut owner,
            cookie,
            frame(&pool, index, 32, |bytes| {
                bytes.fill(index);
                bytes[1] = 0x41;
            }),
            32,
            8,
        )
        .unwrap();
    }
    let original = prepared(&mut owner, cookie).unwrap();
    let committed = owner.buffer_addresses;
    complete_and_detach(&mut owner, cookie, rate);

    // A mask past the batch changes nothing.
    assert_eq!(
        owner.retain_frames(cookie, 0b1011, 0, drop),
        Err(HtAmpduTxError::FrameIndexOutOfRange { index: 3, count: 3 })
    );
    assert_eq!(owner.state(), TxDmaState::Completed);
    // No frame went on the air: every frame stays, none gains its Retry bit.
    owner.retain_frames(cookie, 0b111, 0, drop).unwrap();
    assert_eq!(prepared(&mut owner, cookie).unwrap(), original);
    assert_eq!(owner.state(), TxDmaState::Reserved);
    assert_eq!(owner.buffer_addresses[..3], committed[..3]);
    owner.cancel(cookie, drop).unwrap();
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
        crate::mac::tx::HeMcs::Mcs9,
        crate::mac::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns,
    );
    let density = HtAmpduDensity::EightMicroseconds;
    commit(
        &mut owner,
        cookie,
        frame(&pool, 0, 32, |bytes| bytes.fill(0x5a)),
        32,
        8,
    )
    .unwrap();
    let aggregate = prepared_he(&mut owner, cookie, initial, density).unwrap();
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

    owner.retain_frames(cookie, 0b1, 0b1, drop).unwrap();
    assert_eq!(owner.frame_count(), 1);
    assert_eq!(
        owner.prepared_empty_delimiters(cookie, 0).unwrap(),
        initial
            .ampdu_empty_delimiters(32 + 8 + TX_FCS_SIZE, density)
            .unwrap()
    );
    owner.cancel(cookie, drop).unwrap();
    let buffer = released_slot(&pool, 0, TX_AMPDU_METADATA_SIZE + 2);
    assert_eq!(buffer[TX_AMPDU_METADATA_SIZE + 1] & 0x08, 0x08);
}

#[test]
fn full_window_and_byte_ceiling_are_independent() {
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<32>::new());
    let pool = frame_pool::<1700, 32>();
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    owner.configure_max_aggregate_bytes(0x7fff).unwrap();
    let cookie = owner.begin().unwrap();
    for slot in 0..20 {
        commit(
            &mut owner,
            cookie,
            frame(&pool, slot, 1600, |_| {}),
            1600,
            8,
        )
        .unwrap();
    }
    assert_eq!(owner.frame_count(), 20);
    assert_eq!(
        commit(&mut owner, cookie, frame(&pool, 20, 1600, |_| {}), 1600, 8),
        Err(HtAmpduTxError::AggregateFull)
    );
    assert_eq!(owner.frame_count(), 20);
    owner.cancel(cookie, drop).unwrap();

    owner.configure_max_aggregate_bytes(0x1fff).unwrap();
    let cookie = owner.begin().unwrap();
    for slot in 0..32 {
        commit(&mut owner, cookie, frame(&pool, slot, 100, |_| {}), 100, 8).unwrap();
    }
    assert_eq!(owner.frame_count(), 32);
    owner.cancel(cookie, drop).unwrap();
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

use oer_protocols_ieee80211_core_upper_mac::block_ack::acknowledges;

#[test]
fn block_ack_bitmap_handles_sequence_wrap() {
    let ack = TxBlockAckBitmap::new(SequenceNumber::new(0x0ffe).unwrap(), 0b1101);
    assert!(acknowledges(
        ack.report(),
        SequenceNumber::new(0x0ffe).unwrap()
    ));
    assert!(!acknowledges(
        ack.report(),
        SequenceNumber::new(0x0fff).unwrap()
    ));
    assert!(acknowledges(ack.report(), SequenceNumber::new(0).unwrap()));
    assert!(acknowledges(ack.report(), SequenceNumber::new(1).unwrap()));
    assert!(!acknowledges(ack.report(), SequenceNumber::new(2).unwrap()));
}

#[test]
fn block_ack_bitmap_accepts_only_bounded_sequences_left_of_an_advanced_ssn() {
    let ack = TxBlockAckBitmap::new(SequenceNumber::new(105).unwrap(), 0b1);
    assert!(acknowledges(
        ack.report(),
        SequenceNumber::new(105).unwrap()
    ));
    assert!(acknowledges(
        ack.report(),
        SequenceNumber::new(104).unwrap()
    ));
    assert!(acknowledges(
        ack.report(),
        SequenceNumber::new(100).unwrap()
    ));
    assert!(!acknowledges(
        ack.report(),
        SequenceNumber::new(106).unwrap()
    ));
    assert!(!acknowledges(
        ack.report(),
        SequenceNumber::new(40).unwrap()
    ));

    let wrapped = TxBlockAckBitmap::new(SequenceNumber::new(0).unwrap(), 0b1);
    assert!(acknowledges(
        wrapped.report(),
        SequenceNumber::new(0x0fff).unwrap()
    ));
    assert!(!acknowledges(
        wrapped.report(),
        SequenceNumber::new(65).unwrap()
    ));
}

mod work;

mod exchange_budget;

#[test]
fn no_effect_keeps_the_aggregate_and_its_trigger_reservation_published() {
    use crate::mac::tx::CompletionObservation;
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<2>::new());
    let cookie = storage.as_mut().begin().unwrap();
    let reservation = MacHeTbLinkReservation::for_queue(MacHeTbTidLimit::Three, 2, 2).unwrap();
    {
        let storage = storage.as_mut().project();
        *storage.state = TxDmaState::HardwareOwned;
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
    assert_eq!(storage.state(), TxDmaState::HardwareOwned);
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
    assert_eq!(storage.state(), TxDmaState::Completed);
    assert_eq!(hardware.cleared, Some(reservation));
    assert_eq!(cookie, storage.active);
}
