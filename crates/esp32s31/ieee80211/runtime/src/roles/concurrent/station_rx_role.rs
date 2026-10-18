#![expect(
    clippy::manual_async_fn,
    reason = "the station RX role keeps its borrowed Future service contract explicit"
)]

//! Connected-station RX role for a common STA+AP DATAPATH owner.

#[cfg(test)]
use oer_esp32s31_ieee80211_mac::rx::pool::VENDOR_LARGE_RX_SLOT_COUNT;

use core::future::{Future, ready};

use crate::{
    datapath::{
        DatapathRxProgress,
        network::DatapathNetworkRx,
        rx::{
            ethernet::{PackedEthernetWriter, record_at},
            staging::{StagedEthernetPublication, StagedRxDisposition},
        },
    },
    roles::station::rx_protocol::{
        ConnectedRxProcessor, ConnectedRxProtocolSink, StagedRxAdmission,
    },
};

use oer_esp32s31_ieee80211_mac::rx::pool::VENDOR_LARGE_RX_PAYLOAD_CAPACITY;

use oer_esp32s31_ieee80211_sta::connected_rx::{
    ConnectedRxDispatch, ConnectedRxEvent, ConnectedRxSink,
};

use oer_network_interface::{FrameLengthError, RxEnqueueError};

use super::{StaApStationRxRole, StagedRxFrame};

/// Finite station publication failure at the paired DATAPATH boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApStationRxError {
    BatchCapacity,
    Network(FrameLengthError),
}

/// Role-local decoded Ethernet batch plus the station control observer.
///
/// This sink owns no network publisher. It lets the protocol processor finish
/// one staging lease and retains only copied Ethernet facts until the common
/// DATAPATH lends the addressed station endpoint.
pub struct StaApStationRxSink<'storage, O> {
    storage: &'storage mut [u8],
    used: usize,
    offset: usize,
    failed: bool,
    observer: O,
}

impl<'storage, O> StaApStationRxSink<'storage, O> {
    pub fn new(storage: &'storage mut [u8], observer: O) -> Self {
        assert!(
            storage.len() >= VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
            "paired STA Ethernet batch must cover one complete staged RX unit"
        );
        Self {
            storage,
            used: 0,
            offset: 0,
            failed: false,
            observer,
        }
    }

    pub const fn observer(&self) -> &O {
        &self.observer
    }

    pub fn observer_mut(&mut self) -> &mut O {
        &mut self.observer
    }

    pub const fn has_pending(&self) -> bool {
        self.failed || self.offset != self.used
    }

    /// Return the role-local batch allocation and fact observer after the
    /// paired protocol processor has stopped.
    pub fn into_parts(self) -> (&'storage mut [u8], O) {
        (self.storage, self.observer)
    }

    fn retain_ethernet(&mut self, frame: oer_ieee80211_mac::data::EthernetFrameParts<'_>) {
        if frame.ether_type == 0x888e {
            return;
        }
        let result =
            PackedEthernetWriter::resume(self.storage, self.used).and_then(|mut writer| {
                writer.push(frame)?;
                self.used = writer.used();
                Ok(())
            });
        if result.is_err() {
            self.failed = true;
        }
    }

    fn publish_pending(
        &mut self,
        network: &mut dyn DatapathNetworkRx,
    ) -> Result<DatapathRxProgress, StaApStationRxError> {
        if self.failed {
            return Err(StaApStationRxError::BatchCapacity);
        }
        while let Some(record) = record_at(self.storage, self.used, self.offset)
            .map_err(|_| StaApStationRxError::BatchCapacity)?
        {
            match network.try_send_parts(record.frame) {
                Ok(()) => self.offset = record.next_offset,
                Err(RxEnqueueError::PoolExhausted)
                    if network.pool_exhaustion()
                        == crate::datapath::network::RxPoolExhaustion::DropFrame =>
                {
                    self.offset = record.next_offset;
                }
                Err(
                    RxEnqueueError::QueueFull
                    | RxEnqueueError::PoolExhausted
                    | RxEnqueueError::LinkDown,
                ) => {
                    return Ok(DatapathRxProgress::NetworkBackpressured);
                }
                Err(RxEnqueueError::InvalidLength(error)) => {
                    return Err(StaApStationRxError::Network(error));
                }
            }
        }
        self.used = 0;
        self.offset = 0;
        Ok(DatapathRxProgress::Drained)
    }
}

impl<O: ConnectedRxSink> ConnectedRxSink for StaApStationRxSink<'_, O> {
    fn wants_power_save_data(&self) -> bool {
        self.observer.wants_power_save_data()
    }

    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::Ethernet { frame, .. } = event {
            self.retain_ethernet(frame);
        }
        self.observer.publish(event);
    }

    fn supports_esp_now_v2(&self) -> bool {
        self.observer.supports_esp_now_v2()
    }

    fn publish_esp_now_v2(
        &mut self,
        received: oer_ieee80211_softmac::EspNowReceivedV2<'_>,
        metadata: oer_ieee80211_softmac::MacRxMetadata<oer_esp32s31_ieee80211_mac::rx::RxPhyInfo>,
    ) {
        self.observer.publish_esp_now_v2(received, metadata);
    }
}

impl<O: ConnectedRxSink, const CAPACITY: usize, const SLOTS: usize>
    ConnectedRxProtocolSink<CAPACITY, SLOTS> for StaApStationRxSink<'_, O>
{
    fn staged_rx_admission(&self) -> StagedRxAdmission {
        StagedRxAdmission::AwaitCapacity
    }

    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        ready(())
    }

    fn wait_staged_ready(&mut self) -> impl Future<Output = ()> + '_ {
        ready(())
    }

    fn publish_staged(
        &mut self,
        frame: StagedRxFrame<'_, CAPACITY, SLOTS>,
        ethernet: StagedEthernetPublication,
    ) -> StagedRxDisposition {
        {
            let raw = frame.segment().buffer;
            let payload =
                &raw[ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length];
            self.publish(ConnectedRxEvent::Ethernet {
                frame: oer_ieee80211_mac::data::EthernetFrameParts {
                    destination: ethernet.destination,
                    source: ethernet.source,
                    ether_type: ethernet.ether_type,
                    payload,
                },
                raw,
                amsdu: false,
                metadata: ethernet.metadata,
            });
        }
        drop(frame);
        StagedRxDisposition::Released
    }
}

impl<'pool, M, O, K, const CAPACITY: usize, const SLOTS: usize, const REORDER_SLOTS: usize>
    StaApStationRxRole<'pool, CAPACITY, SLOTS>
    for ConnectedRxProcessor<
        '_,
        'pool,
        '_,
        '_,
        M,
        StaApStationRxSink<'_, O>,
        K,
        CAPACITY,
        SLOTS,
        REORDER_SLOTS,
    >
where
    M: embassy_sync::blocking_mutex::raw::RawMutex,
    O: ConnectedRxSink,
    K: oer_time::Timer,
{
    type Dispatch = Option<ConnectedRxDispatch>;
    type Error = StaApStationRxError;

    fn publish_pending_rx(
        &mut self,
        network: &mut dyn DatapathNetworkRx,
    ) -> Result<DatapathRxProgress, Self::Error> {
        self.sink_mut().publish_pending(network)
    }

    fn service_station_rx<'a>(
        &'a mut self,
        frame: StagedRxFrame<'pool, CAPACITY, SLOTS>,
        _network: &'a mut dyn DatapathNetworkRx,
    ) -> impl Future<Output = Result<Self::Dispatch, Self::Error>> + 'a
    where
        'pool: 'a,
    {
        async move {
            debug_assert!(!self.sink().has_pending());
            Ok(self.dispatch_frame(frame).await)
        }
    }

    fn has_pending_rx(&self) -> bool {
        self.sink().has_pending()
    }
}

#[cfg(test)]
mod tests;
