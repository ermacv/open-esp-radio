//! HIL translation over the image's shared atomic counter bank. Correlation
//! state belongs to the station context; observation retains no history.
use crate::*;
use core::cell::Cell;
use oer_hil_agent::capture::{Reader, collector::Collector};
use oer_hil_protocol::{Message, RequestIdentity, capture::*};
use oer_ieee80211_instrumentation as observation;
use oer_trace::{Event, Observer};

pub trait Publish {
    fn publish<M: Message>(&self, request: RequestIdentity, message: &M);
}
pub struct Observations<'a, const K: usize> {
    collector: &'a Collector<K>,
    identity: Identity,
    key_id: Cell<u8>,
    arp_base: Cell<u32>,
    departing_base: Cell<Option<u32>>,
    arp: Cell<Option<([u8; 4], [u8; 4])>>,
}
impl<'a, const K: usize> Observations<'a, K> {
    pub const fn new(collector: &'a Collector<K>, identity: Identity) -> Self {
        Self {
            collector,
            identity,
            key_id: Cell::new(0),
            arp_base: Cell::new(0),
            departing_base: Cell::new(None),
            arp: Cell::new(None),
        }
    }
    fn count<E: Event>(&self) -> u32 {
        self.collector.count::<E>()
    }
    pub fn departing(&self) {
        self.departing_base
            .set(Some(self.count::<observation::ControlDelivered>()));
    }
    pub fn counters(&self, epoch: u32, collisions: u32, irq_free: u32) -> StationCounters {
        StationCounters {
            epoch,
            tx_timeouts: self.count::<observation::TxHardwareTimeout>(),
            queue_collisions: collisions,
            rx_refused: self.count::<NetworkRefused>(),
            deauth_transmissions: self.departing_base.get().map_or(0, |base| {
                self.count::<observation::ControlDelivered>()
                    .saturating_sub(base)
            }),
            group_generation: self.count::<observation::GroupKeyInstalled>(),
            group_key_id: self.key_id.get(),
            group_arp_received: self
                .count::<NetworkArp>()
                .saturating_sub(self.arp_base.get()),
            irq_free,
        }
    }
    pub fn event<E: Event + Copy + 'static>(
        &self,
        event: E,
        request: RequestIdentity,
        epoch: u32,
        output: &impl Publish,
    ) {
        Observer::event(self, event);
        match observation::Observation::of(&event) {
            Some(observation::Observation::StationAssociated(_)) => output.publish(
                request,
                &StationLink {
                    epoch,
                    state: StationLinkState::Associated,
                },
            ),
            Some(observation::Observation::GroupKeyInstalled(key)) => {
                self.key_id.set(key.key_id);
                self.arp_base.set(self.count::<NetworkArp>());
                output.publish(
                    request,
                    &StationGroupKey {
                        epoch,
                        generation: self.count::<observation::GroupKeyInstalled>(),
                        key_id: key.key_id,
                        replaced: key.replaced,
                    },
                );
            }
            Some(observation::Observation::ArpAddresses(addresses)) => {
                self.arp.set(Some((addresses.sender, addresses.target)))
            }
            Some(observation::Observation::ArpTransmitted(arp)) => self.arp(
                request,
                epoch,
                false,
                true,
                arp.operation,
                arp.sequence,
                arp.packet_number,
                0,
                output,
            ),
            Some(observation::Observation::ArpReceived(arp)) => self.arp(
                request,
                epoch,
                arp.group_addressed,
                false,
                arp.operation,
                arp.sequence,
                arp.packet_number,
                arp.key_id,
                output,
            ),
            Some(observation::Observation::RxFrameDropped(dropped)) => {
                use observation::{FrameDrop, RxDrop, SecurityDrop};
                let reason = match dropped.reason {
                    RxDrop::Frame(FrameDrop::Integrity) => StationReceiveDropReason::Integrity,
                    RxDrop::Frame(FrameDrop::Replay) => StationReceiveDropReason::Replay,
                    RxDrop::Frame(FrameDrop::Protection) => StationReceiveDropReason::Protection,
                    RxDrop::Security(SecurityDrop::InvalidMic) => {
                        StationReceiveDropReason::InvalidMic
                    }
                    RxDrop::Security(SecurityDrop::StaleReplay) => {
                        StationReceiveDropReason::StaleReplay
                    }
                    RxDrop::Security(SecurityDrop::Other) => StationReceiveDropReason::Security,
                    _ => return,
                };
                output.publish(request, &StationReceiveDrop { epoch, reason });
            }
            _ => {}
        }
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "the dictionary's complete public ARP identity"
    )]
    fn arp(
        &self,
        request: RequestIdentity,
        epoch: u32,
        group_addressed: bool,
        transmitted: bool,
        operation: observation::ArpOperation,
        sequence: u16,
        packet_number: u64,
        key_id: u8,
        output: &impl Publish,
    ) {
        if let Some((sender, target)) = self.arp.take() {
            output.publish(
                request,
                &StationArpFrame {
                    epoch,
                    generation: self.count::<observation::GroupKeyInstalled>(),
                    transmitted,
                    group_addressed,
                    operation: match operation {
                        observation::ArpOperation::Request => 1,
                        observation::ArpOperation::Reply => 2,
                    },
                    sender,
                    target,
                    sequence,
                    packet_number,
                    key_id,
                },
            );
        }
    }
}
impl<const K: usize> Observer for Observations<'_, K> {
    fn event<E: Event + Copy + 'static>(&self, event: E) {
        self.collector.event(event);
    }
}
impl<const K: usize> Reader for Observations<'_, K> {
    fn describe(&self) -> Result<Description, Error> {
        self.collector.reader(self.identity).describe()
    }
    fn records(&self, request: PageRequest) -> Result<Records, Error> {
        self.collector.reader(self.identity).records(request)
    }
    fn counters(&self, request: PageRequest) -> Result<Counters, Error> {
        self.collector.reader(self.identity).counters(request)
    }
}

// Platform dictionary kinds local to this image binding: validated bridge
// deliveries/refusals, separate from Wi-Fi's post-decryption observations.
#[derive(Clone, Copy)]
pub struct NetworkArp;
#[derive(Clone, Copy)]
pub struct NetworkRefused;
macro_rules! network_event {
    ($name:ident, $id:literal) => {
        impl Event for $name {
            const KIND: oer_trace::Kind = oer_trace::Kind::new(oer_trace::Domain::Platform, $id);
            const CHANNEL: oer_trace::Channel = const {
                match oer_trace::Channel::new(oer_trace::Domain::Platform, 6) {
                    Ok(channel) => channel,
                    Err(_) => panic!("fixed platform channel"),
                }
            };
            fn encode(&self) -> [u32; 2] {
                [0, 0]
            }
            fn decode(words: [u32; 2]) -> Option<Self> {
                (words == [0, 0]).then_some(Self)
            }
        }
    };
}
network_event!(NetworkArp, 200);
network_event!(NetworkRefused, 201);

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    struct Output;
    impl Publish for Output {
        fn publish<M: Message>(&self, _: RequestIdentity, _: &M) {}
    }
    #[test]
    fn summary_uses_atomic_counts_and_preserves_group_and_stop_boundaries() {
        let bank = Collector::new([
            NetworkArp::KIND,
            NetworkRefused::KIND,
            observation::GroupKeyInstalled::KIND,
            observation::ControlDelivered::KIND,
            observation::TxHardwareTimeout::KIND,
            observation::TxQueueCollision::KIND,
        ])
        .unwrap();
        let identity = Identity {
            capture: 1,
            epoch: 0,
        };
        let observed = Observations::new(&bank, identity);
        let request = RequestIdentity {
            boot_id: 1,
            session_id: 0,
            request_id: 1,
        };
        Observer::event(&observed, NetworkArp);
        observed.event(
            observation::GroupKeyInstalled {
                key_id: 1,
                replaced: false,
            },
            request,
            2,
            &Output,
        );
        Observer::event(&observed, NetworkArp);
        Observer::event(&observed, NetworkRefused);
        Observer::event(&observed, observation::ControlDelivered);
        observed.departing();
        Observer::event(&observed, observation::ControlDelivered);
        Observer::event(&observed, observation::TxHardwareTimeout);
        let counters = observed.counters(2, 3, 4);
        assert_eq!(counters.group_arp_received, 1);
        assert_eq!(counters.group_generation, 1);
        assert_eq!(counters.tx_timeouts, 1);
        assert_eq!(counters.queue_collisions, 3);
        assert_eq!(counters.deauth_transmissions, 1);
        let cut = observed.describe().unwrap();
        assert_eq!(cut.failure, None);
        assert_eq!(cut.dropped, 0);
        assert_eq!(cut.records, 0);
        assert!(cut.frozen);
        assert!(
            observed
                .records(PageRequest {
                    identity: Identity {
                        capture: 2,
                        epoch: 0
                    },
                    first: 0,
                    count: 1
                })
                .is_err()
        );
    }
}
