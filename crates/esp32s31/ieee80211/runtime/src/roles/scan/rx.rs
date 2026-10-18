#![cfg_attr(
    not(target_arch = "riscv32"),
    expect(
        clippy::result_large_err,
        reason = "no-alloc scan shutdown returns the complete RX frontier"
    )
)]

//! Scan-specific view over the role-neutral ESP32-S31 RX-ring owner.

#![forbid(unsafe_code)]

use crate::datapath::rx::{
    dma::{ReceiveDmaStorage, RxEpochResources, StagedRxProducer, walker_enable_settle},
    frontier::{ReceiveFrontier, RxFrontierError, RxFrontierPhase},
};
use crate::mac_clock::MacClockSnapshot;

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_ieee80211_mac::rx::{
    RxDma, RxDmaBufferAddresses, RxIngressConfig, RxRingError, RxRingHalted, RxRingLive,
    decode_rx_local_timestamp, extract_management,
};

use oer_ieee80211_mac::scan::{ScanObservation, ScanTable};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScanRxProgress {
    pub completed_descriptors: u32,
    pub parsed_management_frames: u32,
    pub inserted_records: u32,
    pub updated_records: u32,
    pub malformed_or_irrelevant_frames: u32,
    pub recycled_descriptors: u32,
    pub reload_pending: bool,
    pub service_probe_pending: bool,
}

/// Optional observer for successfully extracted scan frames.
pub trait ScanFrameObserver {
    fn observe(&mut self, frame: &[u8], rssi: i8, table_outcome: ScanObservation);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopScanFrameObserver;

impl ScanFrameObserver for NoopScanFrameObserver {
    fn observe(&mut self, _frame: &[u8], _rssi: i8, _table_outcome: ScanObservation) {}
}

pub struct ScanObservationContext<'a, O, const RECORDS: usize> {
    channel: u8,
    /// The MAC clock relation when this synchronous observation drain
    /// began; every frame it drains was received before it. `None` for a
    /// clock a later radio start replaced.
    clock: Option<MacClockSnapshot>,
    frame: &'a mut [u8],
    table: &'a mut ScanTable<RECORDS>,
    observer: &'a mut O,
}

impl<'a, O, const RECORDS: usize> ScanObservationContext<'a, O, RECORDS> {
    pub fn new(
        channel: u8,
        clock: Option<MacClockSnapshot>,
        frame: &'a mut [u8],
        table: &'a mut ScanTable<RECORDS>,
        observer: &'a mut O,
    ) -> Self {
        Self {
            channel,
            clock,
            frame,
            table,
            observer,
        }
    }

    /// Record one beacon or Probe Response whose raw receive timestamp is
    /// `stamp`.
    pub fn observe_management_frame(
        &mut self,
        frame: &[u8],
        rssi: i8,
        stamp: Option<u32>,
    ) -> ScanObservation
    where
        O: ScanFrameObserver,
    {
        let received_at = stamp.and_then(|raw| self.clock?.received_at(raw));
        let outcome = self
            .table
            .observe_management(frame, self.channel, rssi, received_at);
        self.observer.observe(frame, rssi, outcome);
        outcome
    }
}

/// Scan policy bound to one role-neutral RX-ring owner.
pub struct ScanRx<
    'storage,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    receive: ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>,
    storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
}

impl<'storage, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    ScanRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    #[cfg(not(target_pointer_width = "32"))]
    pub fn prepare_initial<H: RxDma>(
        hardware: &mut H,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
    ) -> Result<Self, RxRingError> {
        ReceiveFrontier::prepare_initial(hardware, storage, descriptor_base, buffer_addresses)
            .map(|receive| Self { receive, storage })
    }

    #[cfg(target_pointer_width = "32")]
    pub fn prepare_initial<H: RxDma>(
        hardware: &mut H,
        storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
    ) -> Result<Self, RxRingError> {
        ReceiveFrontier::prepare_initial(hardware, storage, descriptor_base, buffer_addresses)
            .map(|receive| Self { receive, storage })
    }

    pub const fn from_halted(
        ring: RxRingHalted<'storage, COUNT>,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Self {
        Self {
            receive: ReceiveFrontier::from_halted(ring),
            storage,
        }
    }

    pub const fn from_live(
        ring: RxRingLive<'storage, COUNT>,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Self {
        Self {
            receive: ReceiveFrontier::from_live(ring),
            storage,
        }
    }

    pub const fn phase(&self) -> RxFrontierPhase {
        self.receive.phase()
    }

    pub fn prepare_initial_or_retry<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), RxFrontierError> {
        if self.phase() == RxFrontierPhase::Live {
            Ok(())
        } else {
            self.receive
                .prepare_initial_or_retry(hardware, self.storage)
        }
    }

    pub fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxFrontierError> {
        if self.phase() == RxFrontierPhase::Live {
            Ok(())
        } else {
            self.receive.start_prepared(hardware)
        }
    }

    pub fn observe_management<H, O, const RECORDS: usize>(
        &mut self,
        hardware: &mut H,
        context: &mut ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<ScanRxProgress, RxFrontierError>
    where
        H: RxDma,
        O: ScanFrameObserver,
    {
        let mut progress = ScanRxProgress::default();
        let ring = self
            .receive
            .service_completed_frontier(hardware, self.storage, |segment| {
                let rssi = segment.buffer[0] as i8;
                match extract_management(
                    core::slice::from_ref(&segment),
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                    context.frame,
                ) {
                    Ok(frame) => {
                        progress.parsed_management_frames =
                            progress.parsed_management_frames.saturating_add(1);
                        let frame = &context.frame[..frame.length];
                        let received_at = decode_rx_local_timestamp(segment.buffer)
                            .and_then(|raw| context.clock?.received_at(raw));
                        let outcome = context.table.observe_management(
                            frame,
                            context.channel,
                            rssi,
                            received_at,
                        );
                        context.observer.observe(frame, rssi, outcome);
                        match outcome {
                            ScanObservation::Inserted { .. } => {
                                progress.inserted_records =
                                    progress.inserted_records.saturating_add(1)
                            }
                            ScanObservation::Updated { .. } => {
                                progress.updated_records =
                                    progress.updated_records.saturating_add(1)
                            }
                            _ => {}
                        }
                    }
                    Err(_) => {
                        progress.malformed_or_irrelevant_frames =
                            progress.malformed_or_irrelevant_frames.saturating_add(1);
                    }
                }
            })?;
        progress.completed_descriptors = ring.completed_descriptors;
        progress.recycled_descriptors = ring.recycled_descriptors;
        progress.reload_pending = ring.reload_pending;
        progress.service_probe_pending = ring.service_probe_pending;
        Ok(progress)
    }

    pub fn park(&self) -> Result<(), RxFrontierError> {
        if self.phase() == RxFrontierPhase::Live {
            Ok(())
        } else {
            Err(RxFrontierError::OwnerUnavailable)
        }
    }

    pub fn prepare_next<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxFrontierError> {
        if self.phase() == RxFrontierPhase::Live {
            Ok(())
        } else {
            self.receive.prepare_next(hardware, self.storage)
        }
    }

    pub fn into_halted(self) -> Result<RxRingHalted<'storage, COUNT>, Self> {
        let Self { receive, storage } = self;
        receive
            .try_into_halted()
            .map_err(|receive| Self { receive, storage })
    }

    pub fn into_live(self) -> Result<RxRingLive<'storage, COUNT>, Self> {
        let Self {
            mut receive,
            storage,
        } = self;
        receive.take_live().map_err(|_| Self { receive, storage })
    }
}

mod running;

pub use running::RunningScanRx;

#[cfg(test)]
mod tests;
