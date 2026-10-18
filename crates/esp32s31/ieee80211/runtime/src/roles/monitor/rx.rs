#![cfg_attr(
    not(target_arch = "riscv32"),
    expect(
        clippy::result_large_err,
        reason = "no-alloc monitor shutdown returns the complete RX frontier"
    )
)]

//! Standalone ESP32-S31 normalized-monitor RX owner.

#![forbid(unsafe_code)]

use crate::datapath::rx::{
    dma::ReceiveDmaStorage,
    frontier::{ReceiveFrontier, RxFrontierError, RxFrontierPhase},
};

use oer_esp32s31_ieee80211_mac::rx::{
    RxDma, RxDmaBufferAddresses, RxIngressConfig, RxPhyInfo, RxRingError, RxRingHalted, RxRingLive,
    view_normalized_rx_frame,
};

use oer_ieee80211_softmac::{
    MonitorDropReason, MonitorFilter, MonitorFrame, MonitorPublishOutcome, MonitorSink,
    WifiStandaloneMonitorPlan,
    interface::{ChannelContextId, MonitorTapPoint},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorConfigError {
    UnsupportedTap(MonitorTapPoint),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorPrepareError {
    Configuration(MonitorConfigError),
    Ring(RxRingError),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MonitorRxProgress {
    pub completed_descriptors: u32,
    pub published_frames: u32,
    pub dropped_frames: u32,
    pub full_drops: u32,
    pub oversized_drops: u32,
    pub filtered_drops: u32,
    pub malformed_frames: u32,
    pub recycled_descriptors: u32,
    pub reload_pending: bool,
    pub service_probe_pending: bool,
}

pub struct MonitorRx<
    'storage,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    receive: ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>,
    storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    channel_context: ChannelContextId,
    filter: MonitorFilter,
}

impl<'storage, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    MonitorRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    #[cfg(not(target_pointer_width = "32"))]
    pub fn prepare_initial<H: RxDma>(
        plan: WifiStandaloneMonitorPlan,
        hardware: &mut H,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
    ) -> Result<Self, MonitorPrepareError> {
        let (channel_context, filter) =
            monitor_context(plan).map_err(MonitorPrepareError::Configuration)?;
        let receive =
            ReceiveFrontier::prepare_initial(hardware, storage, descriptor_base, buffer_addresses)
                .map_err(MonitorPrepareError::Ring)?;
        Ok(Self {
            receive,
            storage,
            channel_context,
            filter,
        })
    }

    /// Rebind the physical RX ring returned by a previous role without
    /// acquiring the shared DMA arena a second time.
    pub fn prepare_halted<H: RxDma>(
        plan: WifiStandaloneMonitorPlan,
        ring: RxRingHalted<'storage, COUNT>,
        hardware: &mut H,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Result<Self, (RxRingHalted<'storage, COUNT>, MonitorPrepareError)> {
        let (channel_context, filter) = match monitor_context(plan) {
            Ok(context) => context,
            Err(error) => {
                return Err((ring, MonitorPrepareError::Configuration(error)));
            }
        };
        let prepared = storage
            .prepare_halted(ring, hardware)
            .map_err(|(ring, error)| (ring, MonitorPrepareError::Ring(error)))?;
        Ok(Self {
            receive: ReceiveFrontier::from_prepared(prepared),
            storage,
            channel_context,
            filter,
        })
    }

    pub fn from_live(
        plan: WifiStandaloneMonitorPlan,
        ring: RxRingLive<'storage, COUNT>,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ) -> Result<Self, (RxRingLive<'storage, COUNT>, MonitorPrepareError)> {
        let (channel_context, filter) = match monitor_context(plan) {
            Ok(context) => context,
            Err(error) => {
                return Err((ring, MonitorPrepareError::Configuration(error)));
            }
        };
        Ok(Self {
            receive: ReceiveFrontier::from_live(ring),
            storage,
            channel_context,
            filter,
        })
    }

    #[cfg(target_pointer_width = "32")]
    pub fn prepare_initial<H: RxDma>(
        plan: WifiStandaloneMonitorPlan,
        hardware: &mut H,
        storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
    ) -> Result<Self, MonitorPrepareError> {
        let (channel_context, filter) =
            monitor_context(plan).map_err(MonitorPrepareError::Configuration)?;
        let receive =
            ReceiveFrontier::prepare_initial(hardware, storage, descriptor_base, buffer_addresses)
                .map_err(MonitorPrepareError::Ring)?;
        Ok(Self {
            receive,
            storage,
            channel_context,
            filter,
        })
    }

    pub const fn phase(&self) -> RxFrontierPhase {
        self.receive.phase()
    }

    #[cfg(test)]
    pub const fn channel_context(&self) -> ChannelContextId {
        self.channel_context
    }

    pub fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxFrontierError> {
        if self.phase() == RxFrontierPhase::Live {
            Ok(())
        } else {
            self.receive.start_prepared(hardware)
        }
    }

    pub fn service<H: RxDma, S: MonitorSink<RxPhyInfo>>(
        &mut self,
        hardware: &mut H,
        sink: &mut S,
    ) -> Result<MonitorRxProgress, RxFrontierError> {
        let mut progress = MonitorRxProgress::default();
        let channel_context = self.channel_context;
        let filter = self.filter;
        let ring = self
            .receive
            .service_completed_frontier(hardware, self.storage, |segment| {
                match view_normalized_rx_frame(
                    &segment,
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                ) {
                    Ok(frame) => {
                        let observation = MonitorFrame {
                            tap: MonitorTapPoint::Normalized,
                            channel_context,
                            bytes: frame.mpdu,
                            metadata: frame.metadata,
                            logical_length: frame.logical_length,
                        };
                        if !filter.accepts(&observation) {
                            progress.dropped_frames = progress.dropped_frames.saturating_add(1);
                            progress.filtered_drops = progress.filtered_drops.saturating_add(1);
                        } else {
                            match sink.try_publish(observation) {
                                MonitorPublishOutcome::Published => {
                                    progress.published_frames =
                                        progress.published_frames.saturating_add(1)
                                }
                                MonitorPublishOutcome::Dropped(reason) => {
                                    progress.dropped_frames =
                                        progress.dropped_frames.saturating_add(1);
                                    let reason_count = match reason {
                                        MonitorDropReason::Full => &mut progress.full_drops,
                                        MonitorDropReason::TooLong => &mut progress.oversized_drops,
                                        MonitorDropReason::Filtered => &mut progress.filtered_drops,
                                    };
                                    *reason_count = reason_count.saturating_add(1);
                                }
                            }
                        }
                    }
                    Err(_) => {
                        progress.malformed_frames = progress.malformed_frames.saturating_add(1);
                    }
                }
            })?;
        progress.completed_descriptors = ring.completed_descriptors;
        progress.recycled_descriptors = ring.recycled_descriptors;
        progress.reload_pending = ring.reload_pending;
        progress.service_probe_pending = ring.service_probe_pending;
        Ok(progress)
    }

    /// Rebuild a halted ring before publishing another capture epoch.
    pub fn prepare_next<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxFrontierError> {
        self.receive.prepare_next(hardware, self.storage)
    }

    pub(crate) fn require_reset(&mut self) {
        self.receive.require_reset();
    }

    pub fn into_live(self) -> Result<RxRingLive<'storage, COUNT>, Self> {
        let Self {
            mut receive,
            storage,
            channel_context,
            filter,
        } = self;
        receive.take_live().map_err(|_| Self {
            receive,
            storage,
            channel_context,
            filter,
        })
    }
}

fn monitor_context(
    plan: WifiStandaloneMonitorPlan,
) -> Result<(ChannelContextId, MonitorFilter), MonitorConfigError> {
    let monitor = plan.monitor();
    if monitor.tap() != MonitorTapPoint::Normalized {
        return Err(MonitorConfigError::UnsupportedTap(monitor.tap()));
    }
    Ok((plan.channel_context(), monitor.filter()))
}
