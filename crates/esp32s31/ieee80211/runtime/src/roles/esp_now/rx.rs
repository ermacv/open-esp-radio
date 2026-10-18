//! Normal-RX DMA owner for one standalone ESP-NOW peer epoch.

use crate::datapath::rx::{
    dma::ReceiveDmaStorage,
    frontier::{ReceiveFrontier, RxFrontierError, RxFrontierPhase},
};

use oer_esp32s31_ieee80211_dma::rx_ring::RxSegment;

use oer_esp32s31_ieee80211_mac::rx::{
    RxDma, RxDmaBufferAddresses, RxIngressConfig, RxRingError, RxRingHalted,
};

use oer_esp32s31_ieee80211_sta::standalone_esp_now_rx::{
    StandaloneEspNowRxDispatch, StandaloneEspNowRxDispatcher, StandaloneEspNowRxSink,
};

use oer_ieee80211_mac::channel::WifiChannel;

use oer_ieee80211_softmac::{EspNowRxEpoch, interface::BoundVirtualInterface};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StandaloneEspNowRxProgress {
    pub completed_descriptors: u32,
    pub received: u32,
    pub duplicates: u32,
    pub ignored: u32,
    pub rejected: u32,
    pub recycled_descriptors: u32,
    pub reload_pending: bool,
    pub service_probe_pending: bool,
}

/// Runtime-facing finite normal-RX lifecycle.
///
/// This narrow trait is the explicit integration hook for alternative bounded
/// staging/storage owners. It cannot expose monitor frames or network data.
pub trait StandaloneEspNowReceive<H, S, const PEERS: usize> {
    type Error;

    fn station(&self) -> BoundVirtualInterface;
    fn home_channel(&self) -> WifiChannel;
    fn peer_snapshot_matches(
        &self,
        protocol: &oer_ieee80211_softmac::EspNowProtocol<PEERS>,
    ) -> bool;
    fn phase(&self) -> RxFrontierPhase;
    fn start(&mut self, hardware: &mut H) -> Result<(), Self::Error>;
    fn service(
        &mut self,
        hardware: &mut H,
        sink: &mut S,
    ) -> Result<StandaloneEspNowRxProgress, Self::Error>;
    /// Try to stop the walker. `Ok(false)` is the qualified transient Busy
    /// ownership edge and must be retried cooperatively.
    fn stop(&mut self, hardware: &mut H) -> Result<bool, Self::Error>;
    /// Rebuild a stopped DMA frontier for the next home-channel receive epoch.
    /// This must fail unless the exact halted ring remains owned by `self`.
    fn prepare_next(&mut self, hardware: &mut H) -> Result<(), Self::Error>;
    fn reset_duplicate_history(&mut self) -> usize;
}

/// Concrete no-allocation normal-RX frontier and ESP-NOW dispatcher.
pub struct StandaloneEspNowRx<
    'storage,
    const PEERS: usize,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    receive: ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>,
    storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    mpdu: &'storage mut [u8; DMA_BUFFER_SIZE],
    dispatcher: StandaloneEspNowRxDispatcher<PEERS>,
}

impl<
    'storage,
    const PEERS: usize,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> StandaloneEspNowRx<'storage, PEERS, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    #[cfg(not(target_pointer_width = "32"))]
    pub fn prepare_initial<H: RxDma>(
        hardware: &mut H,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
        mpdu: &'storage mut [u8; DMA_BUFFER_SIZE],
        epoch: EspNowRxEpoch<PEERS>,
        ingress: RxIngressConfig,
    ) -> Result<Self, RxRingError> {
        let receive =
            ReceiveFrontier::prepare_initial(hardware, storage, descriptor_base, buffer_addresses)?;
        Ok(Self {
            receive,
            storage,
            mpdu,
            dispatcher: StandaloneEspNowRxDispatcher::new(epoch, ingress),
        })
    }

    #[cfg(target_pointer_width = "32")]
    pub fn prepare_initial<H: RxDma>(
        hardware: &mut H,
        storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
        mpdu: &'storage mut [u8; DMA_BUFFER_SIZE],
        epoch: EspNowRxEpoch<PEERS>,
        ingress: RxIngressConfig,
    ) -> Result<Self, RxRingError> {
        let receive =
            ReceiveFrontier::prepare_initial(hardware, storage, descriptor_base, buffer_addresses)?;
        Ok(Self {
            receive,
            storage,
            mpdu,
            dispatcher: StandaloneEspNowRxDispatcher::new(epoch, ingress),
        })
    }

    pub fn prepare_halted<H: RxDma>(
        ring: RxRingHalted<'storage, COUNT>,
        hardware: &mut H,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        mpdu: &'storage mut [u8; DMA_BUFFER_SIZE],
        epoch: EspNowRxEpoch<PEERS>,
        ingress: RxIngressConfig,
    ) -> Result<Self, (RxRingHalted<'storage, COUNT>, RxRingError)> {
        let prepared = storage.prepare_halted(ring, hardware)?;
        Ok(Self {
            receive: ReceiveFrontier::from_prepared(prepared),
            storage,
            mpdu,
            dispatcher: StandaloneEspNowRxDispatcher::new(epoch, ingress),
        })
    }

    pub const fn phase(&self) -> RxFrontierPhase {
        self.receive.phase()
    }

    pub const fn dispatcher(&self) -> &StandaloneEspNowRxDispatcher<PEERS> {
        &self.dispatcher
    }

    pub fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxFrontierError> {
        self.receive.start_prepared(hardware)
    }

    pub fn service<H: RxDma, S: StandaloneEspNowRxSink>(
        &mut self,
        hardware: &mut H,
        sink: &mut S,
    ) -> Result<StandaloneEspNowRxProgress, RxFrontierError> {
        let mut progress = StandaloneEspNowRxProgress::default();
        let dispatcher = &mut self.dispatcher;
        let mpdu = &mut self.mpdu[..];
        let ring = self.receive.service_completed_frontier(
            hardware,
            self.storage,
            |segment: RxSegment<'_>| match dispatcher.dispatch(segment, mpdu, sink) {
                StandaloneEspNowRxDispatch::Received { .. }
                | StandaloneEspNowRxDispatch::V2Received { .. } => {
                    progress.received = progress.received.saturating_add(1)
                }
                StandaloneEspNowRxDispatch::Duplicate { .. }
                | StandaloneEspNowRxDispatch::V2Duplicate { .. } => {
                    progress.duplicates = progress.duplicates.saturating_add(1)
                }
                StandaloneEspNowRxDispatch::Ignored => {
                    progress.ignored = progress.ignored.saturating_add(1)
                }
                StandaloneEspNowRxDispatch::Rejected(_) => {
                    progress.rejected = progress.rejected.saturating_add(1)
                }
            },
        )?;
        progress.completed_descriptors = ring.completed_descriptors;
        progress.recycled_descriptors = ring.recycled_descriptors;
        progress.reload_pending = ring.reload_pending;
        progress.service_probe_pending = ring.service_probe_pending;
        Ok(progress)
    }

    pub fn stop<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxFrontierError> {
        self.receive.stop(hardware)
    }

    pub fn reset_duplicate_history(&mut self) -> usize {
        self.dispatcher.reset_duplicate_history()
    }

    pub fn prepare_next<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxFrontierError> {
        self.receive.prepare_next(hardware, self.storage)
    }

    #[allow(clippy::result_large_err)]
    pub fn into_halted(
        self,
    ) -> Result<
        (
            RxRingHalted<'storage, COUNT>,
            &'storage mut [u8; DMA_BUFFER_SIZE],
            EspNowRxEpoch<PEERS>,
        ),
        Self,
    > {
        let Self {
            receive,
            storage,
            mpdu,
            dispatcher,
        } = self;
        match receive.try_into_halted() {
            Ok(ring) => Ok((ring, mpdu, dispatcher.into_epoch())),
            Err(receive) => Err(Self {
                receive,
                storage,
                mpdu,
                dispatcher,
            }),
        }
    }
}

impl<
    H: RxDma,
    S: StandaloneEspNowRxSink,
    const PEERS: usize,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> StandaloneEspNowReceive<H, S, PEERS>
    for StandaloneEspNowRx<'_, PEERS, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    type Error = RxFrontierError;

    fn station(&self) -> BoundVirtualInterface {
        self.dispatcher.epoch().config().station()
    }

    fn home_channel(&self) -> WifiChannel {
        self.dispatcher.epoch().config().home_channel()
    }

    fn peer_snapshot_matches(
        &self,
        protocol: &oer_ieee80211_softmac::EspNowProtocol<PEERS>,
    ) -> bool {
        protocol.owns_rx_epoch(self.dispatcher.epoch())
    }

    fn phase(&self) -> RxFrontierPhase {
        Self::phase(self)
    }

    fn start(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        Self::start(self, hardware)
    }

    fn service(
        &mut self,
        hardware: &mut H,
        sink: &mut S,
    ) -> Result<StandaloneEspNowRxProgress, Self::Error> {
        Self::service(self, hardware, sink)
    }

    fn stop(&mut self, hardware: &mut H) -> Result<bool, Self::Error> {
        match Self::stop(self, hardware) {
            Ok(()) => Ok(true),
            Err(RxFrontierError::Ring(RxRingError::Busy)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn prepare_next(&mut self, hardware: &mut H) -> Result<(), Self::Error> {
        Self::prepare_next(self, hardware)
    }

    fn reset_duplicate_history(&mut self) -> usize {
        Self::reset_duplicate_history(self)
    }
}
