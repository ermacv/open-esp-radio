use crate::rx::storage::{ReceiveDmaStorage, walker_enable_settle};
use oer_esp32s31_ieee80211_dma::space::DmaSpace;

use oer_esp32s31_ieee80211_mac::rx::{
    RxDescriptorSnapshot, RxDma, RxDmaBufferAddresses, RxRingError, RxRingHalted, RxRingLive,
    RxRingStopped,
};

use super::{
    ReceiveFrontier, RxFrontierContinuation, RxFrontierError, RxFrontierIntoLiveFailure,
    RxFrontierPhase, RxFrontierSchedulerSnapshot, state::RxFrontierState,
};

impl<'storage, const COUNT: usize, const DMA_BUFFER_SIZE: usize>
    ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>
{
    pub fn prepare_initial<M: RxDma, const DMA_STORAGE_SIZE: usize, S: DmaSpace>(
        hardware: &mut M,
        storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE, S>,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
    ) -> Result<Self, RxRingError> {
        if DMA_BUFFER_SIZE > u32::MAX as usize {
            return Err(RxRingError::Size);
        }
        storage
            .prepare_ring(hardware, descriptor_base, buffer_addresses)
            .map(Self::from_prepared)
    }

    pub const fn from_halted(ring: RxRingHalted<'storage, COUNT>) -> Self {
        Self {
            state: RxFrontierState::Halted(ring),
        }
    }

    pub const fn from_prepared(ring: RxRingStopped<'storage, COUNT>) -> Self {
        Self {
            state: RxFrontierState::Prepared(ring),
        }
    }

    /// Adopt the continuously running physical frontier of a preceding role.
    pub const fn from_live(ring: RxRingLive<'storage, COUNT>) -> Self {
        Self {
            state: RxFrontierState::Live(ring),
        }
    }

    pub const fn phase(&self) -> RxFrontierPhase {
        self.state.phase()
    }

    /// Prepare a halted ring without starting the DMA walker.
    pub fn prepare_with_storage<M: RxDma, const DMA_STORAGE_SIZE: usize, S: DmaSpace>(
        &mut self,
        hardware: &mut M,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE, S>,
    ) -> Result<(), RxFrontierError> {
        let state = core::mem::replace(&mut self.state, RxFrontierState::Vacant);
        match state {
            RxFrontierState::Halted(halted) => match storage.prepare_halted(halted, hardware) {
                Ok(prepared) => {
                    self.state = RxFrontierState::Prepared(prepared);
                    Ok(())
                }
                Err((halted, error)) => {
                    self.state = RxFrontierState::Halted(halted);
                    Err(error.into())
                }
            },
            prepared @ RxFrontierState::Prepared(_) => {
                self.state = prepared;
                Ok(())
            }
            live @ RxFrontierState::Live(_) => {
                self.state = live;
                Err(RxFrontierError::AlreadyStarted)
            }
            RxFrontierState::Vacant => Err(RxFrontierError::OwnerUnavailable),
        }
    }

    /// Publish an already prepared ring after the role owner has satisfied
    /// its executor-specific settle edge.
    pub fn start_prepared<M: RxDma>(&mut self, hardware: &mut M) -> Result<(), RxFrontierError> {
        let state = core::mem::replace(&mut self.state, RxFrontierState::Vacant);
        let RxFrontierState::Prepared(prepared) = state else {
            self.state = state;
            return Err(RxFrontierError::OwnerUnavailable);
        };
        match prepared.try_start(hardware) {
            Ok(live) => {
                self.state = RxFrontierState::Live(live);
                Ok(())
            }
            Err((prepared, error)) => {
                self.state = RxFrontierState::Prepared(prepared);
                Err(error.into())
            }
        }
    }

    pub fn prepare_initial_or_retry<M: RxDma, const DMA_STORAGE_SIZE: usize, S: DmaSpace>(
        &mut self,
        hardware: &mut M,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE, S>,
    ) -> Result<(), RxFrontierError> {
        self.prepare_with_storage(hardware, storage)
    }

    /// Move the current frontier into another finite phase while leaving an
    /// explicit vacant placeholder for its eventual returned owner.
    pub fn take(&mut self) -> Result<Self, RxFrontierError> {
        let state = core::mem::replace(&mut self.state, RxFrontierState::Vacant);
        if matches!(state, RxFrontierState::Vacant) {
            return Err(RxFrontierError::OwnerUnavailable);
        }
        Ok(Self { state })
    }

    /// Start RX using the production DMA storage bound to this ring.
    pub async fn start_with_storage<M, const DMA_STORAGE_SIZE: usize, S: DmaSpace>(
        &mut self,
        timer: &impl oer_base_time::Timer,
        hardware: &mut M,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE, S>,
    ) -> Result<(), RxFrontierError>
    where
        M: RxDma,
    {
        let state = core::mem::replace(&mut self.state, RxFrontierState::Vacant);
        let prepared = match state {
            RxFrontierState::Halted(halted) => match storage.prepare_halted(halted, hardware) {
                Ok(prepared) => prepared,
                Err((halted, error)) => {
                    self.state = RxFrontierState::Halted(halted);
                    return Err(error.into());
                }
            },
            RxFrontierState::Prepared(prepared) => prepared,
            live @ RxFrontierState::Live(_) => {
                self.state = live;
                return Err(RxFrontierError::AlreadyStarted);
            }
            RxFrontierState::Vacant => {
                return Err(RxFrontierError::OwnerUnavailable);
            }
        };
        walker_enable_settle(timer).await;
        match prepared.try_start(hardware) {
            Ok(live) => {
                self.state = RxFrontierState::Live(live);
                Ok(())
            }
            Err((prepared, error)) => {
                self.state = RxFrontierState::Prepared(prepared);
                Err(error.into())
            }
        }
    }

    pub fn stop<M: RxDma>(&mut self, hardware: &mut M) -> Result<(), RxFrontierError> {
        let state = core::mem::replace(&mut self.state, RxFrontierState::Vacant);
        let RxFrontierState::Live(live) = state else {
            self.state = state;
            return Err(RxFrontierError::OwnerUnavailable);
        };
        match live.try_stop(hardware) {
            Ok(halted) => {
                self.state = RxFrontierState::Halted(halted);
                Ok(())
            }
            Err((live, error)) => {
                self.state = RxFrontierState::Live(live);
                Err(error.into())
            }
        }
    }

    /// Poison a live arena before retaining this owner for board reset.
    ///
    /// This is a terminal quarantine marker, not proof of walker quiescence:
    /// it does not stop DMA or return the descriptor storage for reuse. The
    /// caller must retain the owner until the normal stop/reset boundary.
    /// Other frontier phases remain unchanged.
    pub fn require_reset(&mut self) {
        if let RxFrontierState::Live(ring) = &mut self.state {
            ring.require_reset();
        }
    }

    pub fn prepare_next<M: RxDma, const DMA_STORAGE_SIZE: usize, S: DmaSpace>(
        &mut self,
        hardware: &mut M,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE, S>,
    ) -> Result<(), RxFrontierError> {
        self.prepare_with_storage(hardware, storage)
    }

    pub fn live_mut(&mut self) -> Result<&mut RxRingLive<'storage, COUNT>, RxFrontierError> {
        match &mut self.state {
            RxFrontierState::Live(ring) => Ok(ring),
            _ => Err(RxFrontierError::OwnerUnavailable),
        }
    }

    /// Read one descriptor image in any owned, non-vacant RX phase.
    ///
    /// This is observation only: callers receive no descriptor mutation or
    /// DMA-publication authority. It allows lifecycle fault reports to retain
    /// the terminal hardware image after the walker has been stopped.
    pub fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        match &self.state {
            RxFrontierState::Halted(ring) => ring.descriptor_snapshot(index),
            RxFrontierState::Prepared(ring) => ring.descriptor_snapshot(index),
            RxFrontierState::Live(ring) => ring.descriptor_snapshot(index),
            RxFrontierState::Vacant => None,
        }
    }

    /// Snapshot the live software frontier without exposing publication
    /// authority. AP fault evidence retains this before stop consumes the
    /// live owner and discards scheduler bookkeeping.
    pub fn scheduler_snapshot(&self) -> Option<RxFrontierSchedulerSnapshot> {
        let RxFrontierState::Live(ring) = &self.state else {
            return None;
        };
        Some(RxFrontierSchedulerSnapshot {
            recycle_start: ring.recycle_start().get(),
            accepted_tail: ring.accepted_tail().get(),
            observed_mask: ring.observed_mask(),
            topology: ring.topology_snapshot(),
        })
    }

    /// Whether a published append still needs its finite reload suffix.
    ///
    /// An exhausted walker cannot raise another RX interrupt while this edge
    /// is pending. Role runners must therefore poll the owner again instead
    /// of treating the absence of a new interrupt as an empty receive ring.
    pub const fn reload_pending(&self) -> bool {
        match &self.state {
            RxFrontierState::Live(ring) => ring.reload_pending(),
            _ => false,
        }
    }

    /// Select the next RX scheduler edge from the complete live-ring state.
    ///
    /// Complete vendor `datapathProcessRxSucDataAll` processes through its saved
    /// LAST descriptor, refreshes LAST and continues in the same `ppTask`
    /// invocation. A terminal descriptor writeback or a deferred link-release
    /// proof therefore cannot require another RX-success interrupt: none is
    /// guaranteed after `NEXT=0`.
    pub fn service_continuation<M: RxDma>(
        &mut self,
        hardware: &mut M,
    ) -> Result<RxFrontierContinuation, RxFrontierError> {
        let ring = self.live_mut()?;
        let probe_pending = ring.reload_pending()
            || ring.completion_release_probe_pending()
            // RX_DONE can become visible before LAST advances far enough to
            // authorize recycling. The current PP event already covers that
            // writeback; waiting for another interrupt would lose the vendor
            // worker's same-invocation frontier refresh.
            || ring.completed_descriptor_frontier().descriptor_count != 0
            || ring.exhausted_terminal_writeback_pending(hardware);
        Ok(if probe_pending {
            RxFrontierContinuation::ProbePending
        } else {
            RxFrontierContinuation::AwaitInterrupt
        })
    }

    pub fn take_live(&mut self) -> Result<RxRingLive<'storage, COUNT>, RxFrontierError> {
        let state = core::mem::replace(&mut self.state, RxFrontierState::Vacant);
        match state {
            RxFrontierState::Live(ring) => Ok(ring),
            state => {
                self.state = state;
                Err(RxFrontierError::OwnerUnavailable)
            }
        }
    }

    /// Consume a quiescent pre-connected owner and recover its halted ring.
    ///
    /// A prepared ring has not started DMA and can be demoted synchronously.
    /// Live or vacant owners are returned unchanged: a caller must never use
    /// this conversion as a substitute for stopping the walker.
    #[allow(
        clippy::result_large_err,
        reason = "failure returns the complete affine frontier without allocation"
    )]
    pub fn try_into_halted(self) -> Result<RxRingHalted<'storage, COUNT>, Self> {
        match self.state {
            RxFrontierState::Halted(ring) => Ok(ring),
            RxFrontierState::Prepared(ring) => Ok(ring.into_halted()),
            state => Err(Self { state }),
        }
    }

    /// Consume a live frontier without changing hardware state.
    #[allow(
        clippy::result_large_err,
        reason = "failure returns the complete affine frontier without allocation"
    )]
    pub fn try_into_live(mut self) -> Result<RxRingLive<'storage, COUNT>, Self> {
        match self.take_live() {
            Ok(ring) => Ok(ring),
            Err(_) => Err(self),
        }
    }

    /// Consume the finite protocol owner and materialize a connected RX epoch.
    ///
    /// A live pre-connected frontier is transferred without touching the
    /// walker, NEXT/LAST or descriptor contents. Only a cold halted/prepared
    /// frontier is started here. This keeps physical descriptor credit
    /// continuous while logical scan/join/WPA2 ownership changes.
    ///
    /// The complete pre-connected owner is returned on every failure. The
    /// application therefore cannot strand a halted or prepared ring
    /// between the WPA2 and connected phases.
    #[allow(clippy::result_large_err)]
    pub async fn try_into_live_with_storage<M, const DMA_STORAGE_SIZE: usize, S: DmaSpace>(
        mut self,
        timer: &impl oer_base_time::Timer,
        hardware: &mut M,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE, S>,
    ) -> Result<
        RxRingLive<'storage, COUNT>,
        RxFrontierIntoLiveFailure<'storage, COUNT, DMA_BUFFER_SIZE>,
    >
    where
        M: RxDma,
    {
        if self.phase() == RxFrontierPhase::Live {
            return match self.take_live() {
                Ok(ring) => Ok(ring),
                Err(error) => Err(RxFrontierIntoLiveFailure { owner: self, error }),
            };
        }
        if let Err(error) = self.start_with_storage(timer, hardware, storage).await {
            return Err(RxFrontierIntoLiveFailure { owner: self, error });
        }
        match self.take_live() {
            Ok(ring) => Ok(ring),
            Err(error) => Err(RxFrontierIntoLiveFailure { owner: self, error }),
        }
    }
}
