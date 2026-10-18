//! ESP32-S31 WPA2 bindings for the Embassy RX and ordinary-TX owners.
//!
//! The executor-independent handshake/key ports live in the chip STA crate.
//! This module only adapts the concrete retained DMA frontier and control-TX
//! owner used by the Embassy integration.

use crate::datapath::rx::{
    dma::ReceiveDmaStorage,
    frontier::{ReceiveFrontier, RxFrontierDirective, RxFrontierError},
};

use oer_esp32s31_ieee80211_mac::rx::{RxDma, RxIngressConfig, extract_data};

use oer_esp32s31_ieee80211_sta::wpa2::{Wpa2Receive, Wpa2Station, copy_station_eapol};

use oer_ieee80211_rsn::runner::RsnRxProgress;

/// Retained RX owner bound to its stable DMA allocation for WPA2.
pub struct Wpa2Rx<
    'storage,
    D,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    owner: ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>,
    storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    station: Wpa2Station,
    /// The timer the walker settles on when the ring restarts.
    timer: D,
}

impl<'storage, D, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    Wpa2Rx<'storage, D, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    pub const fn new(
        owner: ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>,
        storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        station: Wpa2Station,
        timer: D,
    ) -> Self {
        Self {
            owner,
            storage,
            station,
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
> Wpa2Receive<H> for Wpa2Rx<'storage, D, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    D: oer_time::Timer,
    H: RxDma,
{
    type Error = RxFrontierError;

    fn service(
        &mut self,
        hardware: &mut H,
        frame: &mut [u8],
    ) -> Result<RsnRxProgress, Self::Error> {
        let mut eapol = None;
        let progress = self
            .owner
            .service_completed(hardware, self.storage, |segment| {
                let candidate = extract_data(
                    core::slice::from_ref(&segment),
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                    frame,
                )
                .ok()
                .and_then(|data| {
                    copy_station_eapol(frame, data.mpdu.length, data.payload_offset, self.station)
                });
                if let Some(candidate) = candidate {
                    eapol = Some(candidate);
                    RxFrontierDirective::Stop
                } else {
                    RxFrontierDirective::Continue
                }
            })?;
        Ok(match eapol {
            Some(eapol) => RsnRxProgress::eapol(progress.completed, eapol),
            None => RsnRxProgress::drained(progress.completed),
        })
    }

    async fn restart<'a>(&'a mut self, hardware: &'a mut H) -> Result<(), Self::Error> {
        if self.owner.phase() == crate::datapath::rx::frontier::RxFrontierPhase::Live {
            Ok(())
        } else {
            self.owner
                .start_with_storage(&self.timer, hardware, self.storage)
                .await
        }
    }

    fn stop(&mut self, _hardware: &mut H) -> Result<(), Self::Error> {
        if self.owner.phase() == crate::datapath::rx::frontier::RxFrontierPhase::Live {
            Ok(())
        } else {
            Err(RxFrontierError::OwnerUnavailable)
        }
    }
}
