//! Capability-bound A-MPDU metadata, descriptor and backing ownership.

#[cfg(any(test, feature = "test-support"))]
extern crate alloc;

use crate::mac::tx_cost::TxContention;
#[cfg(any(test, feature = "test-support"))]
use alloc::boxed::Box;

use core::{ops::Deref, pin::Pin};

use oer_chips_esp32s31_hal::ieee80211::dma::tx_ampdu_storage::{
    AmpduDmaState, AmpduDmaStorage, AmpduDmaStorageError, PinnedAmpduDmaStorage, PushRefused,
    RetainedAmpduDma, RetainedAmpduDmaStorage, WrittenBuffer,
};
use oer_chips_esp32s31_hal::layouts::{WifiTxMetadataHead, WifiTxMetadataHeadMut};
use oer_chips_esp32s31_hal::memory::dma_static::PlacedBacking;
use oer_chips_esp32s31_hal::types::{
    MacHeTxProgram, MacHtTxProgram, MacLegacyTxProgram, MacTxAbortCause, MacTxDetachOutcome,
    MacTxDetachReason, MacTxPrepareRefused,
};
use oer_protocols_ieee80211_wire::frame::set_retry_bit;

use crate::mac::tx::{
    CompletionObservation, HeAmpduTxConfig, HeSmpduTxConfig, HtAmpduTxConfig, HtTxConfig,
    LegacyTxConfig, LegacyTxQueue, TxAbortEnd, TxCompletion, TxCookie, TxDmaState,
};

use super::{
    AmpduFrameLayout, AmpduFrameSize, HtAmpduHardware, HtAmpduLength, HtAmpduTxCompletion,
    HtAmpduTxError, HtAmpduTxFormat, HtAmpduTxStorage, TX_AMPDU_METADATA_SIZE, TX_FCS_SIZE,
};

/// Idle resources required by the safe external-buffer A-MPDU path.
///
/// Protocol metadata and DMA descriptors deliberately have separate owners:
/// [`HtAmpduTxStorage`] contains recovered MAC semantics, while
/// [`AmpduDmaStorage`] is the only layer allowed to publish raw descriptors.
/// Keeping them in one handoff value prevents reconnect/teardown code from
/// accidentally restoring one half without the other.
pub struct HtAmpduTxResources<'storage, const SLOTS: usize> {
    metadata: Pin<&'storage mut HtAmpduTxStorage<SLOTS>>,
    dma: PinnedAmpduDmaStorage<SLOTS, 0>,
}

impl<'storage, const SLOTS: usize> HtAmpduTxResources<'storage, SLOTS> {
    pub fn new(
        metadata: Pin<&'storage mut HtAmpduTxStorage<SLOTS>>,
        dma: PinnedAmpduDmaStorage<SLOTS, 0>,
    ) -> Result<Self, HtAmpduTxError> {
        if metadata.state() != TxDmaState::Free {
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
        metadata: Pin<&'storage mut HtAmpduTxStorage<SLOTS>>,
    ) -> Result<Self, HtAmpduTxError> {
        let dma = Box::leak(Box::new(AmpduDmaStorage::<SLOTS, 0>::new()));
        Self::new(
            metadata,
            AmpduDmaStorage::pin::<oer_chips_esp32s31_hal::ieee80211::dma::space::HostDma>(
                oer_chips_esp32s31_hal::memory::dma_static::Placed::host(dma),
            )?,
        )
    }
}

impl<const SLOTS: usize> Deref for HtAmpduTxResources<'_, SLOTS> {
    type Target = HtAmpduTxStorage<SLOTS>;

    fn deref(&self) -> &Self::Target {
        self.metadata.as_ref().get_ref()
    }
}

impl<const SLOTS: usize> HtAmpduTxResources<'static, SLOTS> {
    /// Bind the static metadata arena and the placed DMA arena at their
    /// final addresses.
    pub fn pin_static(
        metadata: &'static mut HtAmpduTxStorage<SLOTS>,
        dma: oer_chips_esp32s31_hal::memory::dma_static::Placed<AmpduDmaStorage<SLOTS, 0>>,
    ) -> Result<Self, HtAmpduTxError> {
        Self::new(
            HtAmpduTxStorage::pin_static(metadata),
            AmpduDmaStorage::pin::<oer_chips_esp32s31_hal::ieee80211::dma::space::ChipDma>(dma)?,
        )
    }
}

/// Descriptor owner coupled to every external DMA backing it references.
///
/// Unlike a bare [`HtAmpduTxStorage`], this value cannot return or drop a
/// network allocation while hardware can still follow its descriptor. Safe
/// callers append owned [`PlacedBacking`] leases; release is admitted only
/// after the underlying storage has returned to [`TxDmaState::Free`].
pub struct RetainedDmaAmpduTx<'storage, B: 'storage, const SLOTS: usize> {
    storage: Option<Pin<&'storage mut HtAmpduTxStorage<SLOTS>>>,
    dma: Option<RetainedAmpduDma<'storage, B, SLOTS, 0>>,
}

impl<'storage, B: 'storage, const SLOTS: usize> RetainedDmaAmpduTx<'storage, B, SLOTS> {
    pub fn new(
        resources: HtAmpduTxResources<'storage, SLOTS>,
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
        storage: Pin<&'storage mut HtAmpduTxStorage<SLOTS>>,
        retention: &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
    ) -> Result<Self, HtAmpduTxError> {
        Ok(Self::new(
            HtAmpduTxResources::new_model(storage)?,
            retention,
        ))
    }

    /// Record frame `index`'s MSDU length, which only intake sets.
    #[cfg(test)]
    pub(super) fn set_msdu_length_for_test(&mut self, index: usize, length: u16) {
        if let Some(storage) = self.storage.as_mut() {
            storage.as_mut().project().msdu_lengths[index] = length;
        }
    }

    /// The metadata's state, until the owner's teardown took it.
    pub fn state(&self) -> Option<TxDmaState> {
        self.as_ref().map(|storage| storage.get_ref().state())
    }

    /// The metadata, until the owner's teardown took it.
    pub fn as_ref(&self) -> Option<Pin<&HtAmpduTxStorage<SLOTS>>> {
        self.storage.as_ref().map(Pin::as_ref)
    }

    /// Stable address of the metadata allocation behind this movable owner.
    pub fn metadata_address(&self) -> Option<usize> {
        self.as_ref()
            .map(|storage| core::ptr::from_ref(storage.get_ref()).addr())
    }

    fn metadata_mut(&mut self) -> Result<Pin<&mut HtAmpduTxStorage<SLOTS>>, HtAmpduTxError> {
        self.storage
            .as_mut()
            .map(Pin::as_mut)
            .ok_or(HtAmpduTxError::ResetRequired)
    }

    fn dma_mut(&mut self) -> Result<&mut RetainedAmpduDma<'storage, B, SLOTS, 0>, HtAmpduTxError> {
        self.dma.as_mut().ok_or(HtAmpduTxError::ResetRequired)
    }

    pub fn held_backing_count(&self) -> usize {
        self.dma
            .as_ref()
            .map_or(0, RetainedAmpduDma::held_backing_count)
    }

    /// Whether both the protocol metadata and DMA descriptor arena are idle.
    ///
    /// Teardown must check both halves: a zero backing count alone does not
    /// prove that a partially prepared or quarantined descriptor arena can be
    /// republished into a later radio epoch.
    pub fn is_fully_free(&self) -> bool {
        self.state() == Some(TxDmaState::Free)
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
        let state = self.state().ok_or(HtAmpduTxError::ResetRequired)?;
        if state != TxDmaState::Free
            || self.dma_mut()?.state() != AmpduDmaState::Free
            || self.held_backing_count() != 0
        {
            return Err(HtAmpduTxError::NotFree(state));
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
            HtAmpduTxResources<'storage, SLOTS>,
            &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
        ),
        Self,
    > {
        if self.state() != Some(TxDmaState::Free) || self.held_backing_count() != 0 {
            return Err(self);
        }
        let Some(metadata) = self.storage.take() else {
            return Err(self);
        };
        let Some(dma_owner) = self.dma.take() else {
            self.storage = Some(metadata);
            return Err(self);
        };
        let (dma, retention) = match dma_owner.try_into_parts() {
            Ok(parts) => parts,
            Err(dma_owner) => {
                self.storage = Some(metadata);
                self.dma = Some(dma_owner);
                return Err(self);
            }
        };
        Ok((HtAmpduTxResources { metadata, dma }, retention))
    }

    /// Begin a fresh aggregate in both protocol and descriptor owners.
    pub fn begin(&mut self) -> Result<TxCookie, HtAmpduTxError> {
        self.dma_mut()?.begin()?;
        match self.metadata_mut()?.begin() {
            Ok(cookie) => Ok(cookie),
            Err(error) => {
                // `begin` reserved an empty chain: no lease is retained.
                self.dma_mut()?.cancel(drop)?;
                Err(error)
            }
        }
    }

    pub fn configure_max_aggregate_bytes(
        &mut self,
        max_aggregate_bytes: u16,
    ) -> Result<(), HtAmpduTxError> {
        self.metadata_mut()?
            .configure_max_aggregate_bytes(max_aggregate_bytes)
    }

    /// Cancel the reserved aggregate, handing every committed lease to
    /// `returned` in commit order.
    pub fn cancel(
        &mut self,
        cookie: TxCookie,
        returned: impl FnMut(B),
    ) -> Result<(), HtAmpduTxError> {
        self.metadata_mut()?.cancel(cookie)?;
        if let Err(error) = self.dma_mut()?.cancel(returned) {
            self.quarantine();
            return Err(error.into());
        }
        Ok(())
    }

    fn quarantine(&mut self) {
        if let Some(storage) = self.storage.as_mut() {
            *storage.as_mut().project().state = TxDmaState::ResetRequired;
        }
        if let Some(dma) = self.dma.as_mut() {
            dma.quarantine();
        }
    }
}

impl<B: PlacedBacking, const SLOTS: usize> RetainedDmaAmpduTx<'_, B, SLOTS> {
    /// Publish and start an HT A-MPDU through the lower descriptor owner.
    pub fn submit<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: HtAmpduTxConfig,
    ) -> Result<(), HtAmpduTxError> {
        let (Some(storage), Some(dma)) = (self.storage.as_mut(), self.dma.as_mut()) else {
            return Err(HtAmpduTxError::ResetRequired);
        };
        let prepared = storage
            .as_ref()
            .get_ref()
            .prepared_ht_submission(cookie, config)?;
        let count = usize::from(storage.as_ref().get_ref().count);
        let publication = dma.publish_retained_chain(count)?;
        let queue_index = queue.hardware_index();
        let program = MacHtTxProgram::new(&publication, prepared.parameters).ok_or(
            HtAmpduTxError::TxProgramUnavailable {
                format: super::HtAmpduTxFormat::HtAmpdu,
            },
        )?;
        hardware
            .prepare_bound_ht_tx(&publication, queue_index, program)
            .map_err(prepare_refusal)?;
        hardware
            .start_bound_tx(publication, queue_index, || {
                let metadata = storage.as_mut().project();
                *metadata.queue = queue;
                *metadata.aggregate_length = prepared.aggregate.bytes;
                *metadata.detached = false;
                *metadata.state = TxDmaState::HardwareOwned;
            })
            .map_err(prepare_refusal)?;
        storage.as_mut().project().work.record_publication(
            prepared.aggregate.bytes,
            prepared.aggregate.subframes,
            core::num::NonZeroU32::new(config.rate.nominal_kbps()),
            crate::mac::tx::TxPhyRate::Ht(config.rate).ppdu_timing(),
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
        let (Some(storage), Some(dma)) = (self.storage.as_mut(), self.dma.as_mut()) else {
            return Err(HtAmpduTxError::ResetRequired);
        };
        let prepared = storage
            .as_ref()
            .get_ref()
            .prepared_he_submission(cookie, queue, config)?;
        let count = usize::from(storage.as_ref().get_ref().count);
        let publication = dma.publish_retained_chain(count)?;
        let queue_index = queue.hardware_index();
        let program = MacHeTxProgram::new(&publication, prepared.parameters).ok_or(
            HtAmpduTxError::TxProgramUnavailable {
                format: HtAmpduTxFormat::HeAmpdu,
            },
        )?;
        hardware
            .prepare_bound_he_tx(&publication, queue_index, program)
            .map_err(prepare_refusal)?;
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
        let started = hardware.start_bound_tx(publication, queue_index, || {
            let metadata = storage.as_mut().project();
            *metadata.trigger_reservation = prepared.trigger.map(|trigger| trigger.reservation);
            *metadata.trigger_publication_snapshot = trigger_snapshot;
            *metadata.queue = queue;
            *metadata.aggregate_length = prepared.aggregate.bytes;
            *metadata.detached = false;
            *metadata.state = TxDmaState::HardwareOwned;
        });
        if let Err(refused) = started {
            // Nothing records the trigger queue programmed above: it comes
            // down here, or a later cancel would leave it on.
            if let Some(trigger) = prepared.trigger {
                hardware.clear_he_trigger_based_queue(trigger.reservation);
            }
            return Err(prepare_refusal(refused));
        }
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
        let completion = self.metadata_mut()?.acknowledge_completion(hardware)?;
        if matches!(completion, Some(CompletionObservation::Observed(_)))
            && self.dma_mut()?.mark_completed().is_err()
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
        cause: MacTxAbortCause,
    ) -> Result<bool, HtAmpduTxError> {
        self.metadata_mut()?
            .begin_timeout_abort(hardware, cookie, cause)
    }

    /// Detach a collision-owned aggregate, handing every lease to
    /// `returned` in commit order once the queue confirmed its detach.
    pub fn abort_collision<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
        returned: impl FnMut(B),
    ) -> Result<bool, HtAmpduTxError> {
        let queue = {
            let storage = self
                .as_ref()
                .ok_or(HtAmpduTxError::ResetRequired)?
                .get_ref();
            if storage.state != TxDmaState::HardwareOwned || storage.active != cookie {
                return Err(HtAmpduTxError::Stale);
            }
            storage.queue.hardware_index()
        };
        let descriptor_head = self.dma_mut()?.descriptor_head()?;
        let outcome = {
            let dma = self.dma_mut()?;
            hardware.with_tx_queue_detached(
                queue,
                descriptor_head,
                MacTxDetachReason::Collision,
                |detached| dma.release_aborted(detached, returned),
            )
        };
        match outcome {
            MacTxDetachOutcome::NoEvent => return Ok(false),
            MacTxDetachOutcome::Detached(Ok(())) => {}
            MacTxDetachOutcome::OnAir
            | MacTxDetachOutcome::Failed
            | MacTxDetachOutcome::Detached(Err(_)) => {
                self.quarantine();
                return Err(HtAmpduTxError::DetachFailed);
            }
        }
        if let Some(reservation) = self.metadata_mut()?.project().trigger_reservation.take() {
            hardware.clear_he_trigger_based_queue(reservation);
        }
        self.metadata_mut()?.release();
        Ok(true)
    }

    /// Finish a settled abort, handing every lease to `returned` in commit
    /// order once the queue confirmed its detach; [`TxAbortEnd::OnAir`] keeps
    /// them for the completion.
    pub fn finish_timeout_abort<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
        returned: impl FnMut(B),
    ) -> Result<TxAbortEnd, HtAmpduTxError> {
        let queue = {
            let storage = self
                .as_ref()
                .ok_or(HtAmpduTxError::ResetRequired)?
                .get_ref();
            if storage.state != TxDmaState::HardwareOwned || storage.active != cookie {
                return Err(HtAmpduTxError::Stale);
            }
            storage.queue.hardware_index()
        };
        let descriptor_head = self.dma_mut()?.descriptor_head()?;
        let outcome = {
            let dma = self.dma_mut()?;
            hardware.with_tx_queue_detached(
                queue,
                descriptor_head,
                MacTxDetachReason::Timeout,
                |detached| dma.release_aborted(detached, returned),
            )
        };
        match outcome {
            MacTxDetachOutcome::OnAir => return Ok(TxAbortEnd::OnAir),
            MacTxDetachOutcome::Detached(Ok(())) => {}
            MacTxDetachOutcome::NoEvent
            | MacTxDetachOutcome::Failed
            | MacTxDetachOutcome::Detached(Err(_)) => {
                self.quarantine();
                return Err(HtAmpduTxError::DetachFailed);
            }
        }
        if let Some(reservation) = self.metadata_mut()?.project().trigger_reservation.take() {
            hardware.clear_he_trigger_based_queue(reservation);
        }
        self.metadata_mut()?.release();
        Ok(TxAbortEnd::Detached)
    }

    pub fn detach_completed<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), HtAmpduTxError> {
        let queue = {
            let storage = self
                .as_ref()
                .ok_or(HtAmpduTxError::ResetRequired)?
                .get_ref();
            if storage.state != TxDmaState::Completed || storage.active != cookie {
                return Err(HtAmpduTxError::Stale);
            }
            storage.queue.hardware_index()
        };
        let descriptor_head = self.dma_mut()?.descriptor_head()?;
        let outcome = {
            let dma = self.dma_mut()?;
            hardware.with_tx_queue_detached(
                queue,
                descriptor_head,
                MacTxDetachReason::Completed,
                |detached| dma.mark_detached(detached),
            )
        };
        match outcome {
            MacTxDetachOutcome::Detached(Ok(())) => {
                *self.metadata_mut()?.project().detached = true;
                Ok(())
            }
            MacTxDetachOutcome::NoEvent
            | MacTxDetachOutcome::OnAir
            | MacTxDetachOutcome::Failed
            | MacTxDetachOutcome::Detached(Err(_)) => {
                self.quarantine();
                Err(HtAmpduTxError::DetachFailed)
            }
        }
    }

    /// Release a detached completed aggregate, handing every lease to
    /// `returned` in commit order.
    pub fn release_completed(
        &mut self,
        cookie: TxCookie,
        returned: impl FnMut(B),
    ) -> Result<(), HtAmpduTxError> {
        {
            let storage = self
                .as_ref()
                .ok_or(HtAmpduTxError::ResetRequired)?
                .get_ref();
            if storage.state != TxDmaState::Completed
                || storage.active != cookie
                || !storage.detached
            {
                return Err(HtAmpduTxError::Stale);
            }
        }
        if let Err(error) = self.dma_mut()?.release_detached(returned) {
            self.quarantine();
            return Err(error.into());
        }
        self.metadata_mut()?.release();
        Ok(())
    }

    pub fn require_reset(&mut self, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        self.metadata_mut()?.require_reset(cookie)?;
        self.dma_mut()?.quarantine();
        Ok(())
    }
}

/// Why [`RetainedDmaAmpduTx::intake`] returned its backing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduIntakeRefused {
    /// The writer wrote no frame.
    Declined,
    /// The written length is zero or past the frame region.
    Length,
    /// The batch holds its slots or its octet maximum.
    Full,
    /// The owner refused a lifecycle or range edge.
    Fault(HtAmpduTxError),
}

/// The program of a batch's first frame published as a single MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SingleTxConfig {
    Legacy(LegacyTxConfig),
    Ht(HtTxConfig),
    He(HeSmpduTxConfig),
}

/// The octets behind an MPDU its descriptor always covers: the hardware
/// CCMP MIC and the FCS.
const TRAILER_ROOM: usize = crate::ordinary_tx::TX_CCMP_MIC_SIZE + TX_FCS_SIZE as usize;

impl<B: PlacedBacking, const SLOTS: usize> RetainedDmaAmpduTx<'_, B, SLOTS> {
    /// Retain `backing` as the next MPDU of the reserved batch `cookie` and
    /// bind its descriptor once: `write` receives the region after the
    /// private metadata prefix, short of the room the hardware CCMP MIC and
    /// FCS take, writes the MPDU from its first octet and returns its
    /// length. The MIC room the descriptor covers follows the MPDU's
    /// Protected bit. A refusal returns the backing as `write` left it.
    ///
    /// The backing's region starts at its pool slot, word aligned
    /// (SOURCE: `PinnedTxSlot` is `repr(C, align(16))` with the bytes first,
    /// `portable/memory/src/pinned_tx.rs`), so the prefix lands at the
    /// descriptor's word-aligned buffer address
    /// ([`AmpduFrameLayout::new`]) and the MPDU is not moved.
    pub fn intake(
        &mut self,
        cookie: TxCookie,
        backing: B,
        write: impl FnOnce(&mut [u8]) -> Option<u16>,
    ) -> Result<(), (B, AmpduIntakeRefused)> {
        let (Some(storage), Some(dma)) = (self.storage.as_mut(), self.dma.as_mut()) else {
            return Err((
                backing,
                AmpduIntakeRefused::Fault(HtAmpduTxError::ResetRequired),
            ));
        };
        let pushed = dma.push_written(
            backing,
            storage,
            |storage, region| {
                let end = (region.len() & !3).saturating_sub(TRAILER_ROOM);
                let frame = region
                    .get_mut(TX_AMPDU_METADATA_SIZE..end)
                    .ok_or(AmpduIntakeRefused::Length)?;
                let room = frame.len();
                let length = write(frame).ok_or(AmpduIntakeRefused::Declined)?;
                let length = usize::from(length);
                if length == 0 || length > room {
                    return Err(AmpduIntakeRefused::Length);
                }
                let mic = region
                    .get(TX_AMPDU_METADATA_SIZE..)
                    .and_then(<[u8]>::first_chunk::<2>)
                    .and_then(|field| {
                        oer_protocols_ieee80211_wire::frame::FrameControl::new(u16::from_le_bytes(
                            *field,
                        ))
                    })
                    .is_some_and(|control| control.protected());
                let mic = if mic {
                    crate::ordinary_tx::TX_CCMP_MIC_SIZE as u8
                } else {
                    0
                };
                let layout = AmpduFrameLayout::new(0, AmpduFrameSize::new(length, mic))
                    .ok_or(AmpduIntakeRefused::Length)?;
                let frame = storage
                    .as_ref()
                    .get_ref()
                    .prepare_referenced_frame(cookie, region, layout, 0)
                    .map_err(|error| match error {
                        HtAmpduTxError::AggregateFull | HtAmpduTxError::SlotCapacity => {
                            AmpduIntakeRefused::Full
                        }
                        HtAmpduTxError::FrameTooLong => AmpduIntakeRefused::Length,
                        error => AmpduIntakeRefused::Fault(error),
                    })?;
                let written = WrittenBuffer {
                    offset: 0,
                    capacity: frame.descriptor_capacity(),
                    transfer_length: frame.transfer_length(),
                };
                Ok((written, frame))
            },
            |storage, frame| {
                storage
                    .as_mut()
                    .record_referenced_frame(cookie, frame)
                    .map_err(AmpduIntakeRefused::Fault)
            },
        );
        pushed.map_err(|(backing, refused)| {
            let refused = match refused {
                PushRefused::Written(refused) => refused,
                PushRefused::Storage(error) => AmpduIntakeRefused::Fault(error.into()),
            };
            (backing, refused)
        })
    }

    /// Settle the detached batch `cookie` keeping the frames of `keep`, at
    /// least one, compacted in order to positions `0..k` with their
    /// descriptor images, and hand every other backing to `returned` in
    /// position order. Each kept frame of `retry` (positions before the
    /// compaction) gains its Retry bit in place; its header, sequence number
    /// and packet number stay, as does every other kept frame's header.
    pub fn retain_frames(
        &mut self,
        cookie: TxCookie,
        keep: u64,
        retry: u64,
        returned: impl FnMut(B),
    ) -> Result<(), HtAmpduTxError> {
        let (Some(storage), Some(dma)) = (self.storage.as_mut(), self.dma.as_mut()) else {
            return Err(HtAmpduTxError::ResetRequired);
        };
        let count = {
            let metadata = storage.as_ref().get_ref();
            if metadata.state() != TxDmaState::Completed
                || metadata.active != cookie
                || !metadata.detached
            {
                return Err(HtAmpduTxError::Stale);
            }
            metadata.count
        };
        super::retry::check_frame_mask(keep, count)?;
        let retried = keep & retry;
        let kept = (0..usize::from(count)).filter(|position| {
            u32::try_from(*position)
                .ok()
                .and_then(|position| retried.checked_shr(position))
                .is_some_and(|bits| bits & 1 != 0)
        });
        if retried != 0 {
            // Resolve every kept frame before the first is changed.
            for position in kept.clone() {
                let header = dma
                    .software_region_mut(position)
                    .ok()
                    .and_then(|region| region.get(TX_AMPDU_METADATA_SIZE..))
                    .is_some_and(|frame| frame.len() >= 2);
                if !header {
                    return Err(HtAmpduTxError::BackingUnavailable {
                        index: u8::try_from(position).unwrap_or(u8::MAX),
                    });
                }
            }
            for position in kept {
                if let Some(frame) = dma
                    .software_region_mut(position)
                    .ok()
                    .and_then(|region| region.get_mut(TX_AMPDU_METADATA_SIZE..))
                {
                    set_retry_bit(frame);
                }
            }
        }
        let result = dma
            .retain_detached(keep, returned)
            .map_err(HtAmpduTxError::from)
            .and_then(|_| storage.as_mut().retain_frames(cookie, keep).map(|_| ()));
        if result.is_err() {
            self.quarantine();
        }
        result
    }

    /// Set the reserved batch's per-publication metadata for its format and
    /// return its length: each frame's empty delimiters
    /// (`delimiters(psdu)`, zero for every HT and single publication), and
    /// the first frame's HE S-MPDU container bit.
    pub fn prepare_format(
        &mut self,
        cookie: TxCookie,
        delimiters: impl Fn(u16) -> Option<u8>,
        he_smpdu: bool,
    ) -> Result<HtAmpduLength, HtAmpduTxError> {
        let (Some(storage), Some(dma)) = (self.storage.as_mut(), self.dma.as_mut()) else {
            return Err(HtAmpduTxError::ResetRequired);
        };
        let length =
            storage
                .as_mut()
                .set_empty_delimiters(cookie, delimiters, |position, value| {
                    let place = dma
                        .software_region_mut(position)
                        .ok()
                        .and_then(|region| region.get_mut(4))
                        .ok_or(HtAmpduTxError::BackingUnavailable {
                            index: u8::try_from(position).unwrap_or(u8::MAX),
                        })?;
                    *place = value;
                    Ok(())
                })?;
        let head = dma
            .software_region_mut(0)
            .ok()
            .and_then(|region| region.first_chunk_mut::<{ WifiTxMetadataHead::SIZE }>())
            .ok_or(HtAmpduTxError::BackingUnavailable { index: 0 })?;
        let mut head = WifiTxMetadataHeadMut::new(head);
        let control = head.control();
        if control.he_smpdu() != he_smpdu {
            head.set_control(control.with_he_smpdu(he_smpdu));
        }
        Ok(length)
    }

    /// The MPDU at `position` in any state, for host observation of what a
    /// model publication carries.
    #[cfg(any(test, feature = "test-support"))]
    pub fn observed_frame(&mut self, position: usize) -> Option<&[u8]> {
        let (length, _) = self.frame_size(position)?;
        let region = self.dma.as_mut()?.observed_region_mut(position)?;
        region
            .get(TX_AMPDU_METADATA_SIZE..)?
            .get(..usize::from(length))
    }

    /// The descriptor word bound to the frame at `position`, for host
    /// observation.
    #[cfg(any(test, feature = "test-support"))]
    pub fn bound_word0(
        &self,
        position: usize,
    ) -> Option<oer_chips_esp32s31_hal::layouts::WifiTxDmaDescriptorControl> {
        self.dma.as_ref()?.bound_word0(position)
    }

    /// Whether the first frame's Address 1 is an individual address; a
    /// frame too short to hold it is not.
    pub fn first_receiver_individual(&mut self) -> bool {
        self.dma
            .as_mut()
            .and_then(|dma| dma.software_region_mut(0).ok())
            .and_then(|region| region.get(TX_AMPDU_METADATA_SIZE + 4).copied())
            .is_some_and(|octet| octet & 1 == 0)
    }

    /// The batch's length, when no frame carries an empty delimiter or a
    /// hardware HE-Control field.
    pub fn plain_length(&self) -> Option<super::HtAmpduLength> {
        self.storage.as_ref()?.as_ref().get_ref().plain_length()
    }

    /// The MPDU length and hardware MIC length of the frame at `position`.
    pub fn frame_size(&self, position: usize) -> Option<(u16, u8)> {
        self.storage
            .as_ref()?
            .as_ref()
            .get_ref()
            .frame_size(position)
    }

    /// Publish and start the first frame of the reserved batch `cookie` as
    /// a single MPDU through its bound descriptor; the frames behind it stay
    /// retained, unpublished.
    pub fn submit_single<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: SingleTxConfig,
    ) -> Result<(), HtAmpduTxError> {
        let (Some(storage), Some(dma)) = (self.storage.as_mut(), self.dma.as_mut()) else {
            return Err(HtAmpduTxError::ResetRequired);
        };
        {
            let metadata = storage.as_ref().get_ref();
            if metadata.state() != TxDmaState::Reserved || metadata.active != cookie {
                return Err(HtAmpduTxError::Stale);
            }
            if metadata.count == 0 {
                return Err(HtAmpduTxError::TooFewFrames);
            }
        }
        let publication = dma.publish_retained_chain(1)?;
        let index = queue.hardware_index();
        let unavailable = |format| HtAmpduTxError::TxProgramUnavailable { format };
        let prepared = match config {
            SingleTxConfig::Legacy(config) => {
                let program = MacLegacyTxProgram::new(&publication, config.pac_parameters())
                    .ok_or(unavailable(HtAmpduTxFormat::HtAmpdu))?;
                hardware.prepare_bound_legacy_tx(&publication, index, program)
            }
            SingleTxConfig::Ht(config) => {
                let program = MacHtTxProgram::new(&publication, config.pac_parameters())
                    .ok_or(unavailable(HtAmpduTxFormat::HtAmpdu))?;
                hardware.prepare_bound_ht_tx(&publication, index, program)
            }
            SingleTxConfig::He(config) => {
                if !config.valid() {
                    return Err(unavailable(HtAmpduTxFormat::HeAmpdu));
                }
                let program = MacHeTxProgram::new(&publication, config.pac_parameters())
                    .ok_or(unavailable(HtAmpduTxFormat::HeAmpdu))?;
                hardware.prepare_bound_he_tx(&publication, index, program)
            }
        };
        prepared.map_err(prepare_refusal)?;
        // The checks above hold: this `&mut` lets nothing change them.
        let handed_over = || storage.as_mut().record_single_publication(queue);
        hardware
            .start_bound_tx(publication, index, handed_over)
            .map_err(prepare_refusal)
    }

    /// Sample a single MPDU's completion and synchronize both ownership
    /// state machines.
    pub fn acknowledge_single_completion<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<Option<CompletionObservation<TxCompletion>>, HtAmpduTxError> {
        let completion = self
            .metadata_mut()?
            .acknowledge_single_completion(hardware)?;
        if matches!(completion, Some(CompletionObservation::Observed(_)))
            && self.dma_mut()?.mark_completed().is_err()
        {
            self.quarantine();
            return Err(HtAmpduTxError::ResetRequired);
        }
        Ok(completion)
    }

    /// Detach a collision-owned batch, keeping its backings for a retention
    /// or a release; `false` while no collision is pending.
    pub fn abort_collision_retained<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<bool, HtAmpduTxError> {
        Ok(matches!(
            self.detach_aborted(hardware, cookie, MacTxDetachReason::Collision)?,
            Some(TxAbortEnd::Detached)
        ))
    }

    /// Finish a settled abort, keeping the batch's backings for a retention
    /// or a release; [`TxAbortEnd::OnAir`] keeps the batch on the air for its
    /// completion.
    pub fn finish_timeout_abort_retained<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<TxAbortEnd, HtAmpduTxError> {
        match self.detach_aborted(hardware, cookie, MacTxDetachReason::Timeout)? {
            Some(end) => Ok(end),
            None => {
                self.quarantine();
                Err(HtAmpduTxError::DetachFailed)
            }
        }
    }

    fn detach_aborted<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
        reason: MacTxDetachReason,
    ) -> Result<Option<TxAbortEnd>, HtAmpduTxError> {
        let (Some(storage), Some(dma)) = (self.storage.as_mut(), self.dma.as_mut()) else {
            return Err(HtAmpduTxError::ResetRequired);
        };
        let queue = {
            let metadata = storage.as_ref().get_ref();
            if metadata.state() != TxDmaState::HardwareOwned || metadata.active != cookie {
                return Err(HtAmpduTxError::Stale);
            }
            metadata.queue.hardware_index()
        };
        let descriptor_head = dma.descriptor_head()?;
        let outcome = hardware.with_tx_queue_detached(queue, descriptor_head, reason, |detached| {
            dma.mark_aborted_detached(detached)
        });
        match outcome {
            MacTxDetachOutcome::NoEvent => return Ok(None),
            MacTxDetachOutcome::OnAir => return Ok(Some(TxAbortEnd::OnAir)),
            MacTxDetachOutcome::Detached(Ok(())) => {}
            MacTxDetachOutcome::Failed | MacTxDetachOutcome::Detached(Err(_)) => {
                self.quarantine();
                return Err(HtAmpduTxError::DetachFailed);
            }
        }
        if let Some(reservation) = self.metadata_mut()?.project().trigger_reservation.take() {
            hardware.clear_he_trigger_based_queue(reservation);
        }
        if let Err(error) = self.metadata_mut()?.mark_aborted_detached(cookie) {
            self.quarantine();
            return Err(error);
        }
        Ok(Some(TxAbortEnd::Detached))
    }
}

/// A busy queue is retried; any other refusal stays what it is.
const fn prepare_refusal(refused: MacTxPrepareRefused) -> HtAmpduTxError {
    match refused {
        MacTxPrepareRefused::QueueActive => HtAmpduTxError::QueueActive,
        MacTxPrepareRefused::Fault(fault) => HtAmpduTxError::Prepare(fault),
    }
}

/// Tests read the metadata of a live owner directly.
#[cfg(test)]
impl<B, const SLOTS: usize> Deref for RetainedDmaAmpduTx<'_, B, SLOTS> {
    type Target = HtAmpduTxStorage<SLOTS>;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
            .expect("a live owner holds its metadata")
            .get_ref()
    }
}

impl<B, const SLOTS: usize> Drop for RetainedDmaAmpduTx<'_, B, SLOTS> {
    fn drop(&mut self) {
        // `try_into_resources` took both: nothing is left to settle.
        let (Some(storage), Some(dma)) = (self.storage.as_mut(), self.dma.as_mut()) else {
            return;
        };
        let metadata_state = storage.as_ref().get_ref().state();
        let cookie = storage.as_ref().get_ref().active;
        let detached = storage.as_ref().get_ref().detached;
        let released = match (metadata_state, dma.state()) {
            (TxDmaState::Free, AmpduDmaState::Free) => true,
            (TxDmaState::Reserved, AmpduDmaState::Reserved) => {
                storage.as_mut().cancel(cookie).is_ok() && dma.cancel(drop).is_ok()
            }
            (TxDmaState::Completed, AmpduDmaState::Detached) if detached => {
                dma.release_detached(drop).is_ok()
                    && storage.as_mut().release_completed(cookie).is_ok()
            }
            _ => false,
        };
        if !released {
            self.quarantine();
        }
    }
}
