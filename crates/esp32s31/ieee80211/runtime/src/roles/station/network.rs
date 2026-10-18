//! Bounded connected-RX publication into the Embassy network adapter.

use core::{
    future::{Future, poll_fn},
    marker::PhantomData,
};

use oer_esp32s31_ieee80211_sta::connected_rx::{ConnectedRxEvent, ConnectedRxSink};

#[cfg(feature = "diagnostics")]
use crate::diagnostics::network::{RxNetworkDeliveryEvent, RxNetworkDeliveryObserver};

#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::rx_pipeline::{
    RxNetworkPublicationOutcome, RxPipelineObservation, RxPipelineObserver,
};

use crate::{
    datapath::{
        network::{DatapathNetworkRx, RxBackpressure},
        rx::staging::{StagedEthernetPublication, StagedRxDisposition, StagedRxFrame},
    },
    roles::station::rx_protocol::{ConnectedRxProtocolSink, StagedRxAdmission},
};

/// Copies Ethernet events into the bounded network queue and forwards every
/// semantic event to a protocol observer.
pub struct EmbassyNetConnectedRxSink<'resources, N, O> {
    network: N,
    observer: O,
    _resources: PhantomData<&'resources ()>,
    #[cfg(any(feature = "diagnostics", test))]
    pipeline_observer: Option<&'resources dyn RxPipelineObserver>,
    #[cfg(feature = "diagnostics")]
    delivery_observer: Option<&'resources dyn RxNetworkDeliveryObserver>,
}

impl<'resources, N, O> EmbassyNetConnectedRxSink<'resources, N, O> {
    pub const fn new(network: N, observer: O) -> Self {
        Self {
            network,
            observer,
            _resources: PhantomData,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer: None,
            #[cfg(feature = "diagnostics")]
            delivery_observer: None,
        }
    }
}

impl<'resources, N, O> EmbassyNetConnectedRxSink<'resources, N, O> {
    #[cfg(feature = "diagnostics")]
    pub fn with_delivery_observer(
        mut self,
        #[cfg(feature = "diagnostics")] delivery_observer: Option<
            &'resources dyn RxNetworkDeliveryObserver,
        >,
    ) -> Self {
        self.delivery_observer = delivery_observer;
        self
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_pipeline_observer(mut self, observer: &'resources dyn RxPipelineObserver) -> Self {
        self.pipeline_observer = Some(observer);
        self
    }

    pub const fn observer(&self) -> &O {
        &self.observer
    }

    pub fn observer_mut(&mut self) -> &mut O {
        &mut self.observer
    }

    pub const fn network(&self) -> &N {
        &self.network
    }

    pub fn network_mut(&mut self) -> &mut N {
        &mut self.network
    }
}

impl<N: DatapathNetworkRx, O: ConnectedRxSink> ConnectedRxSink
    for EmbassyNetConnectedRxSink<'_, N, O>
{
    fn wants_power_save_data(&self) -> bool {
        self.observer.wants_power_save_data()
    }

    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::Ethernet {
            frame,
            #[cfg(feature = "diagnostics")]
            raw,
            ..
        } = event
        {
            // Connected-state EAPOL belongs to the WPA2 control owner. It is
            // still forwarded to `observer` below, but must never escape into
            // embassy-net as application traffic.
            if frame.ether_type == 0x888e {
                self.observer.publish(event);
                return;
            }
            #[cfg(any(feature = "diagnostics", test))]
            let publish_started = self.pipeline_observer.map(|observer| observer.now_micros());
            #[cfg(not(feature = "diagnostics"))]
            let result = self.network.try_send_parts(frame);
            #[cfg(feature = "diagnostics")]
            let result = {
                let delivery_observer = self.delivery_observer;
                self.network.try_send_parts_observed(frame, &mut || {
                    if let Some(observer) = delivery_observer {
                        observer.admitted(RxNetworkDeliveryEvent::decoded(frame, Some(raw)));
                    }
                })
            };
            #[cfg(any(feature = "diagnostics", test))]
            let outcome = match result {
                Ok(()) => RxNetworkPublicationOutcome::Enqueued,
                Err(error) => {
                    #[cfg(feature = "diagnostics")]
                    if let Some(observer) = self.delivery_observer {
                        observer.dropped(RxNetworkDeliveryEvent::decoded(frame, Some(raw)), error);
                    }
                    if error == oer_network_interface::RxEnqueueError::PoolExhausted {
                        RxNetworkPublicationOutcome::PoolExhausted
                    } else {
                        RxNetworkPublicationOutcome::Dropped
                    }
                }
            };
            #[cfg(not(any(feature = "diagnostics", test)))]
            let _ = result;
            #[cfg(any(feature = "diagnostics", test))]
            if let (Some(observer), Some(started)) = (self.pipeline_observer, publish_started) {
                observer.observe(RxPipelineObservation::NetworkPublication {
                    bytes: frame.payload.len().saturating_add(14),
                    micros: observer.elapsed_micros_since(started),
                    outcome,
                });
            }
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

impl<N: DatapathNetworkRx, O: ConnectedRxSink> EmbassyNetConnectedRxSink<'_, N, O> {
    /// Hand an admitted staged frame to the network without copying it.
    ///
    /// The protocol observer and delivery diagnostics see the decoded frame
    /// first; the 802.11 prefix is then rewritten into an Ethernet header in
    /// the same DMA buffer, whose slot the network owns from then on.
    fn publish_staged_in_place<const STAGE_CAPACITY: usize, const STAGE_SLOTS: usize>(
        &mut self,
        frame: StagedRxFrame<'_, STAGE_CAPACITY, STAGE_SLOTS>,
        ethernet: StagedEthernetPublication,
    ) -> StagedRxDisposition {
        {
            let raw = frame.segment().buffer;
            let parts = oer_ieee80211_mac::data::EthernetFrameParts {
                destination: ethernet.destination,
                source: ethernet.source,
                ether_type: ethernet.ether_type,
                payload: &raw
                    [ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length],
            };
            #[cfg(feature = "diagnostics")]
            if let Some(observer) = self.delivery_observer {
                observer.admitted(RxNetworkDeliveryEvent::decoded(parts, Some(raw)));
            }
            self.observer.publish(ConnectedRxEvent::Ethernet {
                frame: parts,
                raw,
                amsdu: false,
                metadata: ethernet.metadata,
            });
        }
        #[cfg(any(feature = "diagnostics", test))]
        let publish_started = self.pipeline_observer.map(|observer| observer.now_micros());
        let index = match frame.publish_ethernet_in_place(
            ethernet.destination,
            ethernet.source,
            ethernet.ether_type,
            ethernet.payload_offset,
            ethernet.payload_length,
        ) {
            Ok(index) => index,
            Err(_) => unreachable!(
                "a captured payload lies inside its frame behind at least 14 dead bytes"
            ),
        };
        let result = self.network.publish_in_place(index);
        #[cfg(any(feature = "diagnostics", test))]
        if let (Some(observer), Some(started)) = (self.pipeline_observer, publish_started) {
            observer.observe(RxPipelineObservation::NetworkPublication {
                bytes: ethernet.payload_length.saturating_add(14),
                micros: observer.elapsed_micros_since(started),
                outcome: match result {
                    Ok(()) => RxNetworkPublicationOutcome::Enqueued,
                    Err(oer_network_interface::RxEnqueueError::PoolExhausted) => {
                        RxNetworkPublicationOutcome::PoolExhausted
                    }
                    Err(_) => RxNetworkPublicationOutcome::Dropped,
                },
            });
        }
        #[cfg(not(any(feature = "diagnostics", test)))]
        let _ = result;
        StagedRxDisposition::RetainedByNetwork
    }
}

impl<
    N: DatapathNetworkRx,
    O: ConnectedRxSink,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
> ConnectedRxProtocolSink<STAGE_CAPACITY, STAGE_SLOTS> for EmbassyNetConnectedRxSink<'_, N, O>
{
    fn staged_rx_admission(&self) -> StagedRxAdmission {
        StagedRxAdmission::AwaitCapacity
    }

    async fn wait_ready(&mut self) {
        if self.network.backpressure() == RxBackpressure::WaitForCapacity {
            poll_fn(|context| self.network.poll_ready(context)).await;
        }
    }

    fn wait_staged_ready(&mut self) -> impl Future<Output = ()> + '_ {
        <Self as ConnectedRxProtocolSink<STAGE_CAPACITY, STAGE_SLOTS>>::wait_ready(self)
    }

    fn publish_staged(
        &mut self,
        frame: StagedRxFrame<'_, STAGE_CAPACITY, STAGE_SLOTS>,
        ethernet: StagedEthernetPublication,
    ) -> StagedRxDisposition {
        // Zero-copy: EAPOL stays with the control owner, and the Ethernet
        // header needs the 14 dead 802.11/CCMP/LLC bytes before the payload.
        if ethernet.ether_type != 0x888e
            && ethernet.payload_offset >= 14
            && self.network.admit_in_place()
        {
            return self.publish_staged_in_place(frame, ethernet);
        }
        {
            let raw = frame.segment().buffer;
            let payload =
                &raw[ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length];
            let event = ConnectedRxEvent::Ethernet {
                frame: oer_ieee80211_mac::data::EthernetFrameParts {
                    destination: ethernet.destination,
                    source: ethernet.source,
                    ether_type: ethernet.ether_type,
                    payload,
                },
                raw,
                amsdu: false,
                metadata: ethernet.metadata,
            };
            self.publish(event);
        }
        drop(frame);
        StagedRxDisposition::Released
    }
}

#[cfg(all(test, feature = "owned-network"))]
mod tests;
