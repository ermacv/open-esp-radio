//! Capability-bound A-MPDU metadata, descriptor and backing ownership.

#[cfg(any(test, feature = "test-support"))]
extern crate alloc;

#[cfg(any(test, feature = "test-support"))]
use alloc::boxed::Box;
use oer_ieee80211_softmac::tx_cost::TxContention;

use core::{ops::Deref, pin::Pin};

use oer_esp32s31_hal::types::{
    MacHeTxProgram, MacHtTxProgram, MacTxDetachOutcome, MacTxDetachReason,
};
use oer_esp32s31_ieee80211_dma::tx_ampdu_storage::{
    AmpduDmaState, AmpduDmaStorage, AmpduDmaStorageError, PinnedAmpduDmaStorage, RetainedAmpduDma,
    RetainedAmpduDmaStorage,
};
use oer_ieee80211_upper_mac::ampdu::set_retry_bit;
use oer_memory::StableDmaBacking;

use crate::tx::{
    CompletionObservation, HeAmpduTxConfig, HtAmpduTxConfig, LegacyTxQueue, TxCookie, TxSlotState,
};

use super::{
    AmpduRepublication, HeAmpduFrameRequest, HtAmpduFrameRequest, HtAmpduHardware, HtAmpduLength,
    HtAmpduTxCompletion, HtAmpduTxError, HtAmpduTxFormat, HtAmpduTxStorage,
    RetainedAmpduRetryCompletion, RetainedAmpduRetryCompletionError, TX_AMPDU_METADATA_SIZE,
};

/// Idle resources required by the safe external-buffer A-MPDU path.
///
/// Protocol metadata and DMA descriptors deliberately have separate owners:
/// [`HtAmpduTxStorage`] contains recovered MAC semantics, while
/// [`AmpduDmaStorage`] is the only layer allowed to publish raw descriptors.
/// Keeping them in one handoff value prevents reconnect/teardown code from
/// accidentally restoring one half without the other.
pub struct HtAmpduTxResources<'storage, const SLOTS: usize, const BUFFER_SIZE: usize> {
    metadata: Pin<&'storage mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>>,
    dma: PinnedAmpduDmaStorage<SLOTS, 0>,
}

impl<'storage, const SLOTS: usize, const BUFFER_SIZE: usize>
    HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>
{
    pub fn new(
        metadata: Pin<&'storage mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>>,
        dma: PinnedAmpduDmaStorage<SLOTS, 0>,
    ) -> Result<Self, HtAmpduTxError> {
        if metadata.state() != TxSlotState::Free {
            return Err(HtAmpduTxError::NotFree(metadata.state()));
        }
        if dma.state() != AmpduDmaState::Free {
            return Err(HtAmpduTxError::DmaStorage(AmpduDmaStorageError::State));
        }
        Ok(Self { metadata, dma })
    }

    /// Supply a deterministic descriptor address for host state-machine tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new_model(
        metadata: Pin<&'storage mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>>,
    ) -> Result<Self, HtAmpduTxError> {
        const MODEL_DESCRIPTOR_BASE: u32 = 0x2f00_1000;
        let dma = Box::leak(Box::new(AmpduDmaStorage::<SLOTS, 0>::new()));
        Self::new(
            metadata,
            AmpduDmaStorage::pin_static_model(dma, MODEL_DESCRIPTOR_BASE, 0)?,
        )
    }
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> Deref
    for HtAmpduTxResources<'_, SLOTS, BUFFER_SIZE>
{
    type Target = HtAmpduTxStorage<SLOTS, BUFFER_SIZE>;

    fn deref(&self) -> &Self::Target {
        self.metadata.as_ref().get_ref()
    }
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> HtAmpduTxResources<'static, SLOTS, BUFFER_SIZE> {
    /// Bind both statically allocated target arenas at their final addresses.
    pub fn pin_static(
        metadata: &'static mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>,
        dma: &'static mut AmpduDmaStorage<SLOTS, 0>,
    ) -> Result<Self, HtAmpduTxError> {
        Self::new(
            HtAmpduTxStorage::pin_static(metadata),
            AmpduDmaStorage::pin_static(dma)?,
        )
    }
}

/// Descriptor owner coupled to every external DMA backing it references.
///
/// Unlike a bare [`HtAmpduTxStorage`], this value cannot return or drop a
/// network allocation while hardware can still follow its descriptor. Safe
/// callers append owned [`StableDmaBacking`] leases; release is admitted only
/// after the underlying storage has returned to [`TxSlotState::Free`].
pub struct RetainedDmaAmpduTx<'storage, B: 'storage, const SLOTS: usize, const BUFFER_SIZE: usize> {
    storage: Option<Pin<&'storage mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>>>,
    dma: Option<RetainedAmpduDma<'storage, B, SLOTS, 0>>,
}

impl<'storage, B: 'storage, const SLOTS: usize, const BUFFER_SIZE: usize>
    RetainedDmaAmpduTx<'storage, B, SLOTS, BUFFER_SIZE>
{
    pub fn new(
        resources: HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
        retention: &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
    ) -> Self {
        let HtAmpduTxResources { metadata, dma } = resources;
        Self {
            storage: Some(metadata),
            dma: Some(RetainedAmpduDma::new(dma, retention)),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn new_model(
        storage: Pin<&'storage mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>>,
        retention: &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
    ) -> Result<Self, HtAmpduTxError> {
        Ok(Self::new(
            HtAmpduTxResources::new_model(storage)?,
            retention,
        ))
    }

    pub fn as_ref(&self) -> Pin<&HtAmpduTxStorage<SLOTS, BUFFER_SIZE>> {
        self.storage
            .as_ref()
            .expect("retained DMA owner keeps storage until teardown")
            .as_ref()
    }

    /// Stable address of the metadata allocation behind this movable owner.
    pub fn metadata_address(&self) -> usize {
        core::ptr::from_ref(self.as_ref().get_ref()).addr()
    }

    fn metadata_mut(&mut self) -> Pin<&mut HtAmpduTxStorage<SLOTS, BUFFER_SIZE>> {
        self.storage
            .as_mut()
            .expect("retained DMA owner keeps storage until teardown")
            .as_mut()
    }

    fn dma_mut(&mut self) -> &mut RetainedAmpduDma<'storage, B, SLOTS, 0> {
        self.dma
            .as_mut()
            .expect("retained DMA owner keeps descriptor storage until teardown")
    }

    pub fn held_backing_count(&self) -> usize {
        self.dma
            .as_ref()
            .expect("retained DMA owner keeps descriptor storage until teardown")
            .held_backing_count()
    }

    /// Whether both the protocol metadata and DMA descriptor arena are idle.
    ///
    /// Teardown must check both halves: a zero backing count alone does not
    /// prove that a partially prepared or quarantined descriptor arena can be
    /// republished into a later radio epoch.
    pub fn is_fully_free(&self) -> bool {
        self.state() == TxSlotState::Free
            && self
                .dma
                .as_ref()
                .is_some_and(|dma| dma.state() == AmpduDmaState::Free)
            && self.held_backing_count() == 0
    }

    /// Whether the lower descriptor arena, independently of MAC metadata, is idle.
    pub fn dma_is_free(&self) -> bool {
        self.dma
            .as_ref()
            .is_some_and(|dma| dma.state() == AmpduDmaState::Free)
    }

    /// Confirm that every lease has already crossed a safe release edge.
    pub fn release_free_backings(&mut self) -> Result<(), HtAmpduTxError> {
        if self.state() != TxSlotState::Free
            || self.dma_mut().state() != AmpduDmaState::Free
            || self.held_backing_count() != 0
        {
            return Err(HtAmpduTxError::NotFree(self.state()));
        }
        Ok(())
    }

    /// Quarantine every retained lease when hardware ownership is unknowable.
    pub fn forget_backings(&mut self) {
        self.quarantine();
    }

    /// Recover the complete metadata/descriptor handoff only when idle.
    #[allow(clippy::result_large_err)]
    pub fn try_into_resources(
        mut self,
    ) -> Result<
        (
            HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
            &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
        ),
        Self,
    > {
        if self.state() != TxSlotState::Free || self.held_backing_count() != 0 {
            return Err(self);
        }
        let dma_owner = self
            .dma
            .take()
            .expect("retained DMA owner contains descriptor storage");
        let (dma, retention) = match dma_owner.try_into_parts() {
            Ok(parts) => parts,
            Err(dma_owner) => {
                self.dma = Some(dma_owner);
                return Err(self);
            }
        };
        let metadata = self
            .storage
            .take()
            .expect("retained DMA owner contains teardown metadata");
        Ok((HtAmpduTxResources { metadata, dma }, retention))
    }

    /// Begin a fresh aggregate in both protocol and descriptor owners.
    pub fn begin(&mut self) -> Result<TxCookie, HtAmpduTxError> {
        self.dma_mut().begin()?;
        match self.metadata_mut().begin() {
            Ok(cookie) => Ok(cookie),
            Err(error) => {
                self.dma_mut().cancel()?;
                Err(error)
            }
        }
    }

    pub fn configure_max_aggregate_bytes(
        &mut self,
        max_aggregate_bytes: u16,
    ) -> Result<(), HtAmpduTxError> {
        self.metadata_mut()
            .configure_max_aggregate_bytes(max_aggregate_bytes)
    }

    pub fn cancel(&mut self, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        self.metadata_mut().cancel(cookie)?;
        if let Err(error) = self.dma_mut().cancel() {
            self.quarantine();
            return Err(error.into());
        }
        Ok(())
    }

    fn quarantine(&mut self) {
        if let Some(storage) = self.storage.as_mut() {
            *storage.as_mut().project().state = TxSlotState::ResetRequired;
        }
        if let Some(dma) = self.dma.as_mut() {
            dma.quarantine();
        }
    }
}

impl<B: StableDmaBacking, const SLOTS: usize, const BUFFER_SIZE: usize>
    RetainedDmaAmpduTx<'_, B, SLOTS, BUFFER_SIZE>
{
    /// Observe one hardware completion and apply it to the matching retry
    /// state only after both descriptor owners have detached.
    ///
    /// AP and STA supply different publication policy, but neither role may
    /// reorder these ownership edges or calculate BlockAck against a stale
    /// descriptor count.
    pub fn observe_retry_completion<H: HtAmpduHardware, const CAPACITY: usize>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
        retry: &mut crate::tx::runtime::AmpduRetryState<CAPACITY>,
        now: oer_ieee80211_lower_mac::Ieee80211Stamp,
        block_ack_operational: bool,
    ) -> Result<Option<RetainedAmpduRetryCompletion>, RetainedAmpduRetryCompletionError> {
        retry.validate_stamp(now)?;
        let Some(CompletionObservation::Observed(completion)) =
            self.acknowledge_completion(hardware)?
        else {
            // NoEffect leaves both metadata and DMA owners published.
            return Ok(None);
        };
        self.detach_completed(hardware, cookie)?;
        let subframes = self.frame_count();
        let first_sequence = retry.current_first_sequence();
        let decision = retry.observe(completion, subframes, now, block_ack_operational)?;
        Ok(Some(RetainedAmpduRetryCompletion {
            completion,
            first_sequence,
            subframes,
            decision,
        }))
    }

    /// Borrow a detached completed MPDU from its retained stable lease.
    ///
    /// The descriptor address is treated only as an identity to resolve the
    /// owning lease. It is never converted back into a Rust reference.
    pub fn completed_frame(
        &mut self,
        cookie: TxCookie,
        index: u8,
    ) -> Result<(&[u8], u8), HtAmpduTxError> {
        let layout = self
            .storage
            .as_ref()
            .expect("retained DMA owner keeps storage until teardown")
            .as_ref()
            .get_ref()
            .completed_frame_layout(cookie, index)?;
        let bytes = self
            .dma
            .as_mut()
            .expect("retained DMA owner keeps descriptor storage until teardown")
            .detached_logical_region_mut(usize::from(index), layout.buffer_address, layout.capacity)
            .map_err(|_| HtAmpduTxError::BackingUnavailable { index })?;
        Ok((
            &bytes[layout.frame_start..layout.frame_end],
            layout.hardware_mic_length,
        ))
    }

    /// Retain selected MPDUs for another publication. A retransmission sets
    /// their Retry bit through the allocation owner rather than by
    /// reconstructing slices from descriptor addresses.
    pub fn retain_for_ampdu_retry(
        &mut self,
        cookie: TxCookie,
        retry_mask: u32,
        republication: AmpduRepublication,
    ) -> Result<HtAmpduLength, HtAmpduTxError> {
        let locations = self
            .storage
            .as_ref()
            .expect("retained DMA owner keeps storage until teardown")
            .as_ref()
            .retry_frame_locations(cookie, retry_mask, republication)?;

        // Resolve every selected frame before mutating any of them. A stale
        // address therefore fails closed without a partially rewritten batch.
        {
            let dma = self
                .dma
                .as_mut()
                .expect("retained DMA owner keeps descriptor storage until teardown");
            for location in locations.iter().flatten() {
                if dma
                    .detached_logical_region_mut(
                        location.index,
                        location.buffer_address,
                        location.capacity,
                    )
                    .is_err()
                {
                    return Err(HtAmpduTxError::BackingUnavailable {
                        index: location.index as u8,
                    });
                }
            }
        }

        if republication == AmpduRepublication::Retransmission {
            let dma = self
                .dma
                .as_mut()
                .expect("retained DMA owner keeps descriptor storage until teardown");
            for location in locations.iter().flatten() {
                let bytes = dma
                    .detached_logical_region_mut(
                        location.index,
                        location.buffer_address,
                        location.capacity,
                    )
                    .map_err(|_| HtAmpduTxError::BackingUnavailable {
                        index: location.index as u8,
                    })?;
                set_retry_bit(&mut bytes[TX_AMPDU_METADATA_SIZE..]);
            }
        }
        let mut source_indices = [0_u8; SLOTS];
        let mut retained_count = 0;
        for location in locations.iter().flatten() {
            source_indices[retained_count] = location.index as u8;
            retained_count += 1;
        }
        if let Err(error) = self
            .dma_mut()
            .compact_active_backings(&source_indices[..retained_count])
        {
            // Retry bits may already have changed in retained frame bytes.
            // An impossible metadata/backing disagreement is therefore no
            // longer a recoverable Detached aggregate.
            self.quarantine();
            return Err(error.into());
        }
        if let Err(error) = self.dma_mut().begin_retry() {
            // The logical backing order was committed above. Keep the two
            // aggregate owners inseparable if the expected Detached ->
            // Reserved edge cannot be made.
            self.quarantine();
            return Err(error.into());
        }
        match self.metadata_mut().compact_retry_metadata(locations) {
            Ok(length) => Ok(length),
            Err(error) => {
                self.quarantine();
                Err(error)
            }
        }
    }

    /// Commit one HT frame and retain its stable backing in the same owner.
    pub fn commit_ht(
        &mut self,
        cookie: TxCookie,
        backing: B,
        request: HtAmpduFrameRequest,
    ) -> Result<(), HtAmpduTxError> {
        let (storage, dma) = (
            self.storage
                .as_mut()
                .expect("retained DMA owner keeps storage until teardown"),
            self.dma
                .as_mut()
                .expect("retained DMA owner keeps descriptor storage until teardown"),
        );
        let metadata_index = usize::from(storage.as_ref().get_ref().count);
        let (backing_capability, region) = dma.push_backing_region(backing)?;
        let result = (|| {
            let dma_storage = region
                .get_mut(request.layout().dma_offset()..)
                .ok_or(HtAmpduTxError::FrameTooLong)?;
            storage
                .as_mut()
                .commit_referenced_ht_frame(cookie, dma_storage, request)
        })();
        if let Err(error) = result {
            drop(dma.pop_last_backing(backing_capability)?);
            return Err(error);
        }
        let metadata = storage.as_ref().get_ref();
        let descriptor_result = dma.commit_backing_descriptor(
            &backing_capability,
            metadata.buffer_addresses[metadata_index],
            u32::from(metadata.descriptor_capacities[metadata_index]),
            (TX_AMPDU_METADATA_SIZE as u32) + u32::from(metadata.psdu_lengths[metadata_index]),
        );
        if let Err(error) = descriptor_result {
            *storage.as_mut().project().state = TxSlotState::ResetRequired;
            dma.quarantine();
            return Err(error.into());
        }
        Ok(())
    }

    /// Commit one HE frame under the exact TXOP policy and retain its backing.
    pub fn commit_he(
        &mut self,
        cookie: TxCookie,
        backing: B,
        request: HeAmpduFrameRequest,
    ) -> Result<(), HtAmpduTxError> {
        let (storage, dma) = (
            self.storage
                .as_mut()
                .expect("retained DMA owner keeps storage until teardown"),
            self.dma
                .as_mut()
                .expect("retained DMA owner keeps descriptor storage until teardown"),
        );
        let metadata_index = usize::from(storage.as_ref().get_ref().count);
        let (backing_capability, region) = dma.push_backing_region(backing)?;
        let result = (|| {
            let dma_storage = region
                .get_mut(request.layout().dma_offset()..)
                .ok_or(HtAmpduTxError::FrameTooLong)?;
            storage
                .as_mut()
                .commit_referenced_he_frame(cookie, dma_storage, request)
        })();
        if let Err(error) = result {
            drop(dma.pop_last_backing(backing_capability)?);
            return Err(error);
        }
        let metadata = storage.as_ref().get_ref();
        let descriptor_result = dma.commit_backing_descriptor(
            &backing_capability,
            metadata.buffer_addresses[metadata_index],
            u32::from(metadata.descriptor_capacities[metadata_index]),
            (TX_AMPDU_METADATA_SIZE as u32) + u32::from(metadata.psdu_lengths[metadata_index]),
        );
        if let Err(error) = descriptor_result {
            *storage.as_mut().project().state = TxSlotState::ResetRequired;
            dma.quarantine();
            return Err(error.into());
        }
        Ok(())
    }

    /// Publish and start an HT A-MPDU through the lower descriptor owner.
    pub fn submit<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: HtAmpduTxConfig,
    ) -> Result<(), HtAmpduTxError> {
        let (storage, dma) = (
            self.storage
                .as_mut()
                .expect("retained DMA owner keeps storage until teardown"),
            self.dma
                .as_mut()
                .expect("retained DMA owner keeps descriptor storage until teardown"),
        );
        let prepared = storage
            .as_ref()
            .get_ref()
            .prepared_ht_submission(cookie, config)?;
        let count = usize::from(storage.as_ref().get_ref().count);
        let publication = dma.publish_retained_chain(count)?;
        let queue_index = queue.index();
        let program = MacHtTxProgram::new(&publication, prepared.parameters).ok_or(
            HtAmpduTxError::TxProgramUnavailable {
                format: super::HtAmpduTxFormat::HtAmpdu,
            },
        )?;
        if !hardware.prepare_bound_ht_tx(&publication, queue_index, program) {
            return Err(HtAmpduTxError::QueueActive);
        }
        let metadata = storage.as_mut().project();
        *metadata.queue = queue;
        *metadata.aggregate_length = prepared.aggregate.bytes;
        *metadata.detached = false;
        *metadata.state = TxSlotState::HardwareOwned;
        publication.commit(|dma| hardware.start_bound_ht_tx(dma, queue_index));
        storage.as_mut().project().work.record_publication(
            prepared.aggregate.bytes,
            prepared.aggregate.subframes,
            core::num::NonZeroU32::new(config.rate.nominal_kbps()),
            crate::tx::TxPhyRate::Ht(config.rate).ppdu_timing(),
            Some(TxContention {
                aifsn: config.aifsn,
                backoff_slots: config.contention_window,
            }),
        );
        Ok(())
    }

    /// Publish and start an HE A-MPDU through the lower descriptor owner.
    pub fn submit_he<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: HeAmpduTxConfig,
    ) -> Result<(), HtAmpduTxError> {
        let (storage, dma) = (
            self.storage
                .as_mut()
                .expect("retained DMA owner keeps storage until teardown"),
            self.dma
                .as_mut()
                .expect("retained DMA owner keeps descriptor storage until teardown"),
        );
        let prepared = storage
            .as_ref()
            .get_ref()
            .prepared_he_submission(cookie, queue, config)?;
        let count = usize::from(storage.as_ref().get_ref().count);
        let publication = dma.publish_retained_chain(count)?;
        let queue_index = queue.index();
        let program = MacHeTxProgram::new(&publication, prepared.parameters).ok_or(
            HtAmpduTxError::TxProgramUnavailable {
                format: HtAmpduTxFormat::HeAmpdu,
            },
        )?;
        if !hardware.prepare_bound_he_tx(&publication, queue_index, program) {
            return Err(HtAmpduTxError::QueueActive);
        }
        let mut trigger_snapshot = None;
        if let Some(trigger) = prepared.trigger {
            trigger_snapshot = Some(
                hardware
                    .prepare_he_trigger_based_queue(
                        trigger.policy,
                        trigger.reservation,
                        trigger.tid,
                        &storage.as_ref().get_ref().psdu_lengths[..count],
                        trigger.queued_msdu_bytes,
                    )
                    .map_err(HtAmpduTxError::TriggerBased)?,
            );
        }
        let metadata = storage.as_mut().project();
        *metadata.trigger_reservation = prepared.trigger.map(|trigger| trigger.reservation);
        *metadata.trigger_publication_snapshot = trigger_snapshot;
        *metadata.queue = queue;
        *metadata.aggregate_length = prepared.aggregate.bytes;
        *metadata.detached = false;
        *metadata.state = TxSlotState::HardwareOwned;
        publication.commit(|dma| hardware.start_bound_he_tx(dma, queue_index));
        storage.as_mut().project().work.record_publication(
            prepared.aggregate.bytes,
            prepared.aggregate.subframes,
            None,
            None,
            Some(TxContention {
                aifsn: config.aifsn,
                backoff_slots: config.contention_window,
            }),
        );
        Ok(())
    }

    /// Sample BlockAck and synchronize both ownership state machines.
    pub fn acknowledge_completion<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<Option<CompletionObservation<HtAmpduTxCompletion>>, HtAmpduTxError> {
        let completion = self.metadata_mut().acknowledge_completion(hardware)?;
        if matches!(completion, Some(CompletionObservation::Observed(_)))
            && self.dma_mut().mark_completed().is_err()
        {
            self.quarantine();
            return Err(HtAmpduTxError::ResetRequired);
        }
        Ok(completion)
    }

    pub fn begin_timeout_abort<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<bool, HtAmpduTxError> {
        self.metadata_mut().begin_timeout_abort(hardware, cookie)
    }

    pub fn abort_collision<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<bool, HtAmpduTxError> {
        let queue = {
            let storage = self.as_ref().get_ref();
            if storage.state != TxSlotState::HardwareOwned || storage.active != cookie {
                return Err(HtAmpduTxError::Stale);
            }
            storage.queue.index()
        };
        let descriptor_head = self.dma_mut().descriptor_head();
        let outcome = {
            let dma = self.dma_mut();
            hardware.with_tx_queue_detached(
                queue,
                descriptor_head,
                MacTxDetachReason::Collision,
                |detached| dma.release_aborted(detached),
            )
        };
        match outcome {
            MacTxDetachOutcome::NoEvent => return Ok(false),
            MacTxDetachOutcome::Detached(Ok(())) => {}
            MacTxDetachOutcome::Failed | MacTxDetachOutcome::Detached(Err(_)) => {
                self.quarantine();
                return Err(HtAmpduTxError::DetachFailed);
            }
        }
        if let Some(reservation) = self.metadata_mut().project().trigger_reservation.take() {
            hardware.clear_he_trigger_based_queue(reservation);
        }
        self.metadata_mut().release();
        Ok(true)
    }

    pub fn finish_timeout_abort<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), HtAmpduTxError> {
        let queue = {
            let storage = self.as_ref().get_ref();
            if storage.state != TxSlotState::HardwareOwned || storage.active != cookie {
                return Err(HtAmpduTxError::Stale);
            }
            storage.queue.index()
        };
        let descriptor_head = self.dma_mut().descriptor_head();
        let outcome = {
            let dma = self.dma_mut();
            hardware.with_tx_queue_detached(
                queue,
                descriptor_head,
                MacTxDetachReason::Timeout,
                |detached| dma.release_aborted(detached),
            )
        };
        match outcome {
            MacTxDetachOutcome::NoEvent => return Err(HtAmpduTxError::TimeoutNotPending),
            MacTxDetachOutcome::Detached(Ok(())) => {}
            MacTxDetachOutcome::Failed | MacTxDetachOutcome::Detached(Err(_)) => {
                self.quarantine();
                return Err(HtAmpduTxError::DetachFailed);
            }
        }
        if let Some(reservation) = self.metadata_mut().project().trigger_reservation.take() {
            hardware.clear_he_trigger_based_queue(reservation);
        }
        self.metadata_mut().release();
        Ok(())
    }

    pub fn detach_completed<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), HtAmpduTxError> {
        let queue = {
            let storage = self.as_ref().get_ref();
            if storage.state != TxSlotState::Completed || storage.active != cookie {
                return Err(HtAmpduTxError::Stale);
            }
            storage.queue.index()
        };
        let descriptor_head = self.dma_mut().descriptor_head();
        let outcome = {
            let dma = self.dma_mut();
            hardware.with_tx_queue_detached(
                queue,
                descriptor_head,
                MacTxDetachReason::Completed,
                |detached| dma.mark_detached(detached),
            )
        };
        match outcome {
            MacTxDetachOutcome::Detached(Ok(())) => {
                *self.metadata_mut().project().detached = true;
                Ok(())
            }
            MacTxDetachOutcome::NoEvent
            | MacTxDetachOutcome::Failed
            | MacTxDetachOutcome::Detached(Err(_)) => {
                self.quarantine();
                Err(HtAmpduTxError::DetachFailed)
            }
        }
    }

    pub fn release_completed(&mut self, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        {
            let storage = self.as_ref().get_ref();
            if storage.state != TxSlotState::Completed
                || storage.active != cookie
                || !storage.detached
            {
                return Err(HtAmpduTxError::Stale);
            }
        }
        if let Err(error) = self.dma_mut().release_detached() {
            self.quarantine();
            return Err(error.into());
        }
        self.metadata_mut().release();
        Ok(())
    }

    pub fn require_reset(&mut self, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        self.metadata_mut().require_reset(cookie)?;
        self.dma_mut().quarantine();
        Ok(())
    }
}

impl<B, const SLOTS: usize, const BUFFER_SIZE: usize> Deref
    for RetainedDmaAmpduTx<'_, B, SLOTS, BUFFER_SIZE>
{
    type Target = HtAmpduTxStorage<SLOTS, BUFFER_SIZE>;

    fn deref(&self) -> &Self::Target {
        self.as_ref().get_ref()
    }
}

impl<B, const SLOTS: usize, const BUFFER_SIZE: usize> Drop
    for RetainedDmaAmpduTx<'_, B, SLOTS, BUFFER_SIZE>
{
    fn drop(&mut self) {
        if self.storage.is_none() || self.dma.is_none() {
            return;
        }
        let metadata_state = self.state();
        let dma_state = self.dma_mut().state();
        let released = match (metadata_state, dma_state) {
            (TxSlotState::Free, AmpduDmaState::Free) => true,
            (TxSlotState::Reserved, AmpduDmaState::Reserved) => {
                let cookie = self.active;
                self.metadata_mut().cancel(cookie).is_ok() && self.dma_mut().cancel().is_ok()
            }
            (TxSlotState::Completed, AmpduDmaState::Detached) if self.detached => {
                let cookie = self.active;
                self.dma_mut().release_detached().is_ok()
                    && self.metadata_mut().release_completed(cookie).is_ok()
            }
            _ => false,
        };
        if !released {
            self.quarantine();
        }
    }
}
