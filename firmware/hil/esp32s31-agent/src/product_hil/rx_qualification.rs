//! Observation-only RX evidence for the product HIL composition.
//!
//! The production driver owns classification and delivery. This module only
//! samples borrowed semantic events and never consumes, mutates or delays
//! their delivery to the network sink.

#![forbid(unsafe_code)]

#[cfg(feature = "station-exit-evidence")]
use core::cell::RefCell;

use core::sync::atomic::AtomicU32;
#[cfg(feature = "station-exit-evidence")]
use core::sync::atomic::Ordering;

#[cfg(feature = "station-exit-evidence")]
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
#[cfg(feature = "station-exit-evidence")]
use oer_esp32s31_ieee80211_system::{
    ConnectedRxObservation, ConnectedRxObserver, ReceiveEvidence, RxObservedEthernetFrame,
};
#[cfg(feature = "station-exit-evidence")]
use oer_esp32s31_ieee80211_system::{RxNetworkDeliveryEvent, RxNetworkDeliveryObserver};
#[cfg(feature = "rx-delivery-telemetry")]
use oer_hil_esp32s31_telemetry::rx_delivery::{NetworkDropReason, RxDeliveryTracker};

use oer_hil_esp32s31_telemetry::rx_evidence::{RxAmpduCounters, RxPhyCounters, RxSmpduCounters};
#[cfg(feature = "rx-delivery-telemetry")]
use oer_hil_protocol::{network::RxDeliveryEvidence, network::RxReorderDeliveryEvidence};
#[cfg(feature = "rx-delivery-telemetry")]
use oer_network_interface::FrameLengthError;
#[cfg(feature = "station-exit-evidence")]
use oer_network_interface::RxEnqueueError;

pub(crate) static RX_PHY: RxPhyCounters = RxPhyCounters::new();
pub(crate) static RX_S_MPDU: RxSmpduCounters = RxSmpduCounters::new();
pub(crate) static BEACON_S_MPDU: RxSmpduCounters = RxSmpduCounters::new();
pub(crate) static RX_AMPDU: RxAmpduCounters = RxAmpduCounters::new();
#[cfg(feature = "rx-delivery-telemetry")]
static RX_DELIVERY: Mutex<CriticalSectionRawMutex, RefCell<Option<RxDeliveryTracker<128>>>> =
    Mutex::new(RefCell::new(None));
pub(crate) static LAST_FORMAT: AtomicU32 = AtomicU32::new(u32::MAX);
pub(crate) static LAST_PHY: AtomicU32 = AtomicU32::new(u32::MAX);

#[cfg(feature = "station-exit-evidence")]
static ANOMALIES: Mutex<
    CriticalSectionRawMutex,
    RefCell<oer_hil_esp32s31_telemetry::rx_anomaly::Records<8>>,
> = Mutex::new(RefCell::new(
    oer_hil_esp32s31_telemetry::rx_anomaly::Records::new(),
));
#[cfg(feature = "station-exit-evidence")]
pub(crate) fn begin_anomalies(session: u64) {
    ANOMALIES.lock(|r| r.borrow_mut().begin(session));
    ARP.lock(|r| r.borrow_mut().begin(session));
}
#[cfg(feature = "station-exit-evidence")]
pub(crate) fn end_anomalies(session: u64) -> (Option<u32>, Option<u32>) {
    (
        ARP.lock(|r| r.borrow_mut().end(session)),
        ANOMALIES.lock(|r| r.borrow_mut().end(session)),
    )
}

#[cfg(feature = "station-exit-evidence")]
pub(crate) async fn report_anomalies(session: u64, totals: (Option<u32>, Option<u32>)) {
    use crate::console::runtime_log_reliably;
    #[cfg(feature = "rx-delivery-telemetry")]
    {
        use oer_hil_esp32s31_telemetry::rx_delivery::FORWARD_GAP_SAMPLE_CAPACITY;
        let count = RX_DELIVERY.lock(|r| {
            r.borrow()
                .as_ref()
                .and_then(|r| r.completed_gap_samples(session).map(|(count, _)| count))
        });
        if let Some(count) = count {
            runtime_log_reliably(format_args!(
                "ORX_GAPS session={} correlated={} capacity={}",
                session, count, FORWARD_GAP_SAMPLE_CAPACITY
            ))
            .await;
            for index in 0..FORWARD_GAP_SAMPLE_CAPACITY {
                let sample = RX_DELIVERY.lock(|r| {
                    r.borrow().as_ref().and_then(|r| {
                        r.completed_gap_samples(session)
                            .and_then(|(_, samples)| samples[index])
                    })
                });
                if let Some(sample) = sample {
                    runtime_log_reliably(format_args!(
                        "ORX_GAP session={} index={} {:?}",
                        session, index, sample
                    ))
                    .await;
                }
            }
        }
    }
    if let Some(total) = totals.0 {
        runtime_log_reliably(format_args!(
            "ORX_ARP_SUMMARY session={} total={} capacity=32",
            session, total
        ))
        .await;
        for index in 0..32 {
            let sample = ARP.lock(|r| r.borrow().samples[index]);
            if let Some(sample) = sample {
                runtime_log_reliably(format_args!(
                    "ORX_ARP session={} index={} {:?}",
                    session, index, sample
                ))
                .await;
            }
        }
    }
    if let Some(total) = totals.1 {
        runtime_log_reliably(format_args!(
            "ORX_ANOMALIES session={} total={} capacity=8",
            session, total
        ))
        .await;
        for index in 0..8 {
            let sample = ANOMALIES.lock(|r| r.borrow().samples[index]);
            if let Some(sample) = sample {
                runtime_log_reliably(format_args!(
                    "ORX_ANOMALY session={} index={} {:?}",
                    session, index, sample
                ))
                .await;
            }
        }
    }
}

#[cfg(feature = "station-exit-evidence")]
static ARP: Mutex<
    CriticalSectionRawMutex,
    RefCell<oer_hil_esp32s31_telemetry::arp_frontier::Records>,
> = Mutex::new(RefCell::new(
    oer_hil_esp32s31_telemetry::arp_frontier::Records::new(),
));
#[cfg(feature = "station-exit-evidence")]
fn observe_arp(
    frame: RxObservedEthernetFrame<'_>,
    stage: oer_hil_esp32s31_telemetry::arp_frontier::Stage,
) {
    use oer_hil_esp32s31_telemetry::arp_frontier::{Identity, Sample};
    if let Some(identity) = Identity::parse(frame.ether_type, frame.payload) {
        let sample = Sample {
            at_us: embassy_time::Instant::now().as_micros(),
            stage,
            identity,
            ethernet_destination: frame.destination,
        };
        ARP.lock(|r| r.borrow_mut().observe(sample));
    }
}

#[cfg(feature = "station-exit-evidence")]
pub(crate) struct HilConnectedRxObserver {
    udp_port: u16,
}

#[cfg(feature = "station-exit-evidence")]
impl HilConnectedRxObserver {
    pub(crate) const fn new(udp_port: u16) -> Self {
        Self { udp_port }
    }

    #[cfg(feature = "rx-delivery-telemetry")]
    pub(crate) fn begin_delivery_session(session_id: u64) {
        RX_DELIVERY.lock(|tracker| {
            let mut tracker = tracker.borrow_mut();
            tracker
                .get_or_insert_with(RxDeliveryTracker::new)
                .begin(session_id);
        });
    }

    #[cfg(feature = "rx-delivery-telemetry")]
    pub(crate) fn observe_udp_consumer(session_id: u64, sequence: i32) {
        RX_DELIVERY.lock(|tracker| {
            if let Some(tracker) = tracker.borrow_mut().as_mut() {
                tracker.consumed(session_id, sequence);
            }
        });
    }

    #[cfg(feature = "rx-delivery-telemetry")]
    pub(crate) fn finish_delivery_session(
        session_id: u64,
        reorder: RxReorderDeliveryEvidence,
    ) -> Option<RxDeliveryEvidence> {
        RX_DELIVERY.lock(|tracker| {
            tracker
                .borrow_mut()
                .as_mut()
                .and_then(|tracker| tracker.finish(session_id, reorder))
        })
    }
}

#[cfg(feature = "station-exit-evidence")]
impl ConnectedRxObserver for HilConnectedRxObserver {
    fn requests_phy(&self, frame: RxObservedEthernetFrame<'_>) -> bool {
        // A strict interval vector gate cannot be based on one out of every
        // 64 packets: a fallback vector could otherwise remain invisible.
        // The observer already classifies every benchmark UDP publication;
        // requesting its decoded value adds no raw-prefix ownership or wait.
        ipv4_udp_destination_port(frame) == Some(self.udp_port)
    }

    fn observe(&self, event: ConnectedRxObservation<'_>) {
        match event {
            ConnectedRxObservation::Beacon { s_mpdu } => {
                observe_s_mpdu(&BEACON_S_MPDU, s_mpdu);
            }
            ConnectedRxObservation::Ethernet {
                frame,
                qos_sequence,
                s_mpdu,
                ampdu,
                phy,
            } if ipv4_udp_destination_port(frame) == Some(self.udp_port) => {
                observe_s_mpdu(&RX_S_MPDU, s_mpdu);
                observe_ampdu(&RX_AMPDU, ampdu);
                if let Some(phy) = available(phy) {
                    LAST_FORMAT.store(u32::from(phy.baseband_format), Ordering::Relaxed);
                    let mut packed = u32::from(phy.baseband_format) | (u32::from(phy.rate) << 4);
                    if let Some(signal) = phy.ht {
                        packed |= (1 << 30)
                            | (u32::from(signal.mcs) << 9)
                            | (u32::from(signal.short_guard_interval) << 16)
                            | (u32::from(signal.bandwidth_mhz == 40) << 17);
                        RX_PHY.observe_ht(
                            signal.mcs,
                            signal.bandwidth_mhz,
                            signal.short_guard_interval,
                        );
                    } else if let Some(signal) = phy.he_su {
                        let bandwidth = match signal.bandwidth_mhz {
                            20 => 0,
                            40 => 1,
                            80 => 2,
                            _ => 3,
                        };
                        packed |= (1 << 31)
                            | (u32::from(signal.mcs) << 9)
                            | (u32::from(signal.guard_interval_and_ltf) << 13)
                            | (bandwidth << 15)
                            | (u32::from(signal.dcm) << 17)
                            | (u32::from(signal.ldpc) << 18);
                        RX_PHY.observe_he_mcs(signal.mcs);
                    } else {
                        RX_PHY.observe_other();
                        let sample = oer_hil_esp32s31_telemetry::rx_anomaly::Sample {
                            observed_us: embassy_time::Instant::now().as_micros(),
                            udp_sequence: ipv4_udp_sequence(frame, self.udp_port),
                            ip_bytes: frame.payload.len(),
                            qos: qos_sequence.map(|q| (q.tid, q.sequence)),
                            format: phy.baseband_format,
                            rate: phy.rate,
                            signal_words: phy.signal_words,
                            ampdu: match ampdu {
                                ReceiveEvidence::Unavailable => 0,
                                ReceiveEvidence::Hardware(false) => 1,
                                ReceiveEvidence::Hardware(true) => 2,
                                ReceiveEvidence::Protocol(false) => 3,
                                ReceiveEvidence::Protocol(true) => 4,
                            },
                        };
                        ANOMALIES.lock(|r| r.borrow_mut().observe(sample));
                    }
                    LAST_PHY.store(packed, Ordering::Relaxed);
                }
            }
            ConnectedRxObservation::Ethernet { frame, .. } if frame.ether_type == 0x0806 => {
                observe_arp(
                    frame,
                    oer_hil_esp32s31_telemetry::arp_frontier::Stage::Radio,
                );
            }
            _ => {}
        }
    }
}

#[cfg(feature = "station-exit-evidence")]
impl RxNetworkDeliveryObserver for HilConnectedRxObserver {
    fn admitted(&self, event: RxNetworkDeliveryEvent<'_>) {
        observe_arp(
            event.frame,
            oer_hil_esp32s31_telemetry::arp_frontier::Stage::Admitted,
        );
        #[cfg(feature = "rx-delivery-telemetry")]
        {
            let Some(sequence) = ipv4_udp_sequence(event.frame, self.udp_port) else {
                return;
            };
            RX_DELIVERY.lock(|tracker| {
                if let Some(tracker) = tracker.borrow_mut().as_mut() {
                    tracker.admitted(
                        sequence,
                        event.qos_sequence.map(|qos| (qos.tid, qos.sequence.get())),
                    );
                }
            });
        }
    }
    fn dropped(&self, event: RxNetworkDeliveryEvent<'_>, error: RxEnqueueError) {
        use oer_hil_esp32s31_telemetry::arp_frontier::Stage;
        observe_arp(
            event.frame,
            match error {
                RxEnqueueError::QueueFull => Stage::QueueFull,
                RxEnqueueError::PoolExhausted => Stage::PoolExhausted,
                RxEnqueueError::LinkDown => Stage::LinkDown,
                RxEnqueueError::InvalidLength(_) => Stage::InvalidLength,
            },
        );
        #[cfg(feature = "rx-delivery-telemetry")]
        {
            let Some(sequence) = ipv4_udp_sequence(event.frame, self.udp_port) else {
                return;
            };
            let reason = match error {
                RxEnqueueError::QueueFull => NetworkDropReason::QueueFull,
                RxEnqueueError::PoolExhausted => NetworkDropReason::PoolExhausted,
                RxEnqueueError::LinkDown => NetworkDropReason::LinkDown,
                RxEnqueueError::InvalidLength(
                    FrameLengthError::TooShort | FrameLengthError::TooLong,
                ) => NetworkDropReason::InvalidLength,
            };
            RX_DELIVERY.lock(|tracker| {
                if let Some(tracker) = tracker.borrow_mut().as_mut() {
                    tracker.dropped(
                        sequence,
                        event.qos_sequence.map(|qos| (qos.tid, qos.sequence.get())),
                        reason,
                    );
                }
            });
        }
    }
}

#[cfg(feature = "station-exit-evidence")]
fn ipv4_udp_destination_port(frame: RxObservedEthernetFrame<'_>) -> Option<u16> {
    if frame.ether_type != 0x0800 {
        return None;
    }
    let version_and_ihl = *frame.payload.first()?;
    if version_and_ihl >> 4 != 4 || *frame.payload.get(9)? != 17 {
        return None;
    }
    let header_length = usize::from(version_and_ihl & 0x0f).checked_mul(4)?;
    if header_length < 20 {
        return None;
    }
    Some(u16::from_be_bytes([
        *frame.payload.get(header_length + 2)?,
        *frame.payload.get(header_length + 3)?,
    ]))
}

#[cfg(feature = "station-exit-evidence")]
fn ipv4_udp_sequence(frame: RxObservedEthernetFrame<'_>, destination_port: u16) -> Option<i32> {
    if ipv4_udp_destination_port(frame) != Some(destination_port) {
        return None;
    }
    let header_length = usize::from(*frame.payload.first()? & 0x0f).checked_mul(4)?;
    let sequence_offset = header_length.checked_add(8)?;
    let encoded: [u8; 4] = frame
        .payload
        .get(sequence_offset..sequence_offset + 4)?
        .try_into()
        .ok()?;
    Some(i32::from_be_bytes(encoded))
}

#[cfg(feature = "station-exit-evidence")]
fn observe_s_mpdu(counter: &RxSmpduCounters, evidence: ReceiveEvidence<bool>) {
    match evidence {
        ReceiveEvidence::Hardware(value) => counter.observe_hardware(value),
        ReceiveEvidence::Protocol(_) | ReceiveEvidence::Unavailable => {
            counter.observe_unavailable();
        }
    }
}

#[cfg(feature = "station-exit-evidence")]
fn observe_ampdu(counter: &RxAmpduCounters, evidence: ReceiveEvidence<bool>) {
    match evidence {
        ReceiveEvidence::Hardware(value) => counter.observe_hardware(value),
        ReceiveEvidence::Protocol(value) => counter.observe_protocol(value),
        ReceiveEvidence::Unavailable => counter.observe_unavailable(),
    }
}

#[cfg(feature = "station-exit-evidence")]
fn available<T>(evidence: ReceiveEvidence<T>) -> Option<T> {
    match evidence {
        ReceiveEvidence::Hardware(value) | ReceiveEvidence::Protocol(value) => Some(value),
        ReceiveEvidence::Unavailable => None,
    }
}
