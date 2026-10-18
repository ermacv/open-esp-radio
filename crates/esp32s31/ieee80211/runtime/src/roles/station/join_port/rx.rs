use crate::datapath::rx::{
    dma::ReceiveDmaStorage,
    frontier::{ReceiveFrontier, RxFrontierDirective, RxFrontierError, RxFrontierPhase},
};

use oer_esp32s31_ieee80211_mac::{
    init::MacRuntimeStopHardware,
    rx::{RxDma, RxIngressConfig, RxSegment, extract_management},
};

use oer_esp32s31_ieee80211_sta::join::StaJoinReceive;

use oer_ieee80211_sta::join::{StaJoinRxDirective, StaJoinRxObserver};

/// RX owner bound to the stable DMA storage used by every finite join phase.
pub struct StaJoinRx<
    'storage,
    D,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    owner: ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>,
    storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    /// The timer the walker settles on when the ring restarts.
    timer: D,
}

impl<'storage, D, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    StaJoinRx<'storage, D, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    pub const fn new(
        owner: ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        timer: D,
    ) -> Self {
        Self {
            owner,
            storage,
            timer,
        }
    }

    pub fn into_owner(self) -> ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE> {
        self.owner
    }
}

impl<
    'storage,
    D,
    H,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> StaJoinReceive<H> for StaJoinRx<'storage, D, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    D: oer_time::Timer,
    H: RxDma + MacRuntimeStopHardware,
{
    type Error = RxFrontierError;

    async fn start<'a>(&'a mut self, hardware: &'a mut H) -> Result<(), Self::Error> {
        let started = if self.owner.phase() == RxFrontierPhase::Live {
            Ok(())
        } else {
            self.owner
                .start_with_storage(&self.timer, hardware, self.storage)
                .await
        };
        if started.is_ok() {
            hardware.resume_mac_runtime();
        }
        started
    }

    fn stop(&mut self, _hardware: &mut H) -> Result<(), Self::Error> {
        if self.owner.phase() == RxFrontierPhase::Live {
            Ok(())
        } else {
            Err(RxFrontierError::OwnerUnavailable)
        }
    }

    fn service_management<O>(
        &mut self,
        hardware: &mut H,
        frame: &mut [u8],
        observer: &mut O,
    ) -> Result<(), Self::Error>
    where
        O: StaJoinRxObserver,
    {
        self.owner
            .service_completed(hardware, self.storage, |segment: RxSegment<'_>| {
                let management = extract_management(
                    core::slice::from_ref(&segment),
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                    frame,
                )
                .ok();
                let management = management.map(|parsed| &frame[..parsed.length]);
                match observer.observe_completed(management) {
                    StaJoinRxDirective::Continue => RxFrontierDirective::Continue,
                    StaJoinRxDirective::Stop => RxFrontierDirective::Stop,
                }
            })
            .map(|_| ())
    }
}
