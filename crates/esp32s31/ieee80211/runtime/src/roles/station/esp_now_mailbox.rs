//! Connected-station ESP-NOW receive decorator over the shared mailbox.

use crate::{
    datapath::rx::staging::{StagedEthernetPublication, StagedRxDisposition, StagedRxFrame},
    roles::{esp_now::mailbox::EspNowRxPublisher, station::rx_protocol::ConnectedRxProtocolSink},
};

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_ieee80211_mac::rx::RxPhyInfo;

use oer_esp32s31_ieee80211_sta::connected_rx::{ConnectedRxEvent, ConnectedRxSink};

use oer_ieee80211_softmac::{EspNowReceivedV2, MacRxMetadata};

/// Sink decorator which copies only ESP-NOW events into the dedicated
/// mailbox, then forwards the original borrowed event to the existing sink.
/// Connected BlockAck, security and network behavior therefore remains owned
/// by the already composed sink.
pub struct EspNowMailboxConnectedRxSink<'resources, M: RawMutex, S, const CAPACITY: usize> {
    inner: S,
    publisher: EspNowRxPublisher<'resources, M, CAPACITY>,
}

impl<'resources, M: RawMutex, S, const CAPACITY: usize>
    EspNowMailboxConnectedRxSink<'resources, M, S, CAPACITY>
{
    pub const fn new(inner: S, publisher: EspNowRxPublisher<'resources, M, CAPACITY>) -> Self {
        Self { inner, publisher }
    }

    pub const fn inner(&self) -> &S {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    pub fn into_parts(self) -> (S, EspNowRxPublisher<'resources, M, CAPACITY>) {
        (self.inner, self.publisher)
    }
}

impl<M: RawMutex, S: ConnectedRxSink, const CAPACITY: usize> ConnectedRxSink
    for EspNowMailboxConnectedRxSink<'_, M, S, CAPACITY>
{
    fn wants_power_save_data(&self) -> bool {
        self.inner.wants_power_save_data()
    }

    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::EspNow { received, metadata } = event {
            let _ = self.publisher.try_publish(received, metadata);
        }
        self.inner.publish(event);
    }

    fn supports_esp_now_v2(&self) -> bool {
        true
    }

    fn publish_esp_now_v2(
        &mut self,
        received: EspNowReceivedV2<'_>,
        metadata: MacRxMetadata<RxPhyInfo>,
    ) {
        let _ = self.publisher.try_publish_v2(received, metadata);
        if self.inner.supports_esp_now_v2() {
            self.inner.publish_esp_now_v2(received, metadata);
        }
    }
}

impl<
    M: RawMutex,
    S: ConnectedRxProtocolSink<FRAME_CAPACITY, FRAME_SLOTS>,
    const MAILBOX_CAPACITY: usize,
    const FRAME_CAPACITY: usize,
    const FRAME_SLOTS: usize,
> ConnectedRxProtocolSink<FRAME_CAPACITY, FRAME_SLOTS>
    for EspNowMailboxConnectedRxSink<'_, M, S, MAILBOX_CAPACITY>
{
    fn staged_rx_admission(&self) -> super::rx_protocol::StagedRxAdmission {
        self.inner.staged_rx_admission()
    }

    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        self.inner.wait_ready()
    }

    fn wait_staged_ready(&mut self) -> impl Future<Output = ()> + '_ {
        self.inner.wait_staged_ready()
    }

    fn publish_staged(
        &mut self,
        frame: StagedRxFrame<'_, FRAME_CAPACITY, FRAME_SLOTS>,
        ethernet: StagedEthernetPublication,
    ) -> StagedRxDisposition {
        self.inner.publish_staged(frame, ethernet)
    }
}
