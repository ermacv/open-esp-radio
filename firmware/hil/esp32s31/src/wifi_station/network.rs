//! P1 DHCP/ICMP stack over the existing owned Ethernet adapter.
use super::Attempt;
use core::cell::{Cell, RefCell};
use embassy_futures::{join::join3, select::select, yield_now};
use oer_embassy_net::{
    NetworkInterfaceId, NoopRawMutex, OwnedEndpointResources, OwnedNetworkDevice,
    OwnedNetworkRunner,
};
use oer_embassy_time::EmbassyClock;
use oer_firmware_apps_w3::{icmp, observation::Observations, report::Lease};
use oer_hil_protocol::wifi::StationProbeFinished;
use oer_ieee80211_sta::data::{EthernetFrame, TxMsdu};
use oer_memory::{WakingSpscReceiver, WakingSpscSender};
use oer_ieee80211_wire::{data::EthernetFrameParts, qos::WmmUserPriority};
use oer_time::{Clock, Duration, Timer};
use owned_embassy_net::{
    Stack, StackStorage,
    iface::Iface,
    iface::dhcpv4::DhcpConfig,
    raw::RawSocket,
    wire::{ArpHardware, ArpOperation, ArpPacket, EthernetProtocol, IpProtocol, IpVersion},
};
use static_cell::{ConstStaticCell, StaticCell};
use xarxa_driver::{PacketPool, PacketPoolStorage};

pub const RX: usize = 8;
const TX: usize = 4;
type Endpoint = OwnedEndpointResources<NoopRawMutex, RX, TX>;
type Device = OwnedNetworkDevice<'static, NoopRawMutex, RX, TX>;
type Runner = OwnedNetworkRunner<'static, NoopRawMutex, RX, TX>;
static ENDPOINT: StaticCell<Endpoint> = StaticCell::new();
static DEVICE: StaticCell<Device> = StaticCell::new();
static STACK: StaticCell<StackStorage<'static, Device>> = StaticCell::new();
static GENERAL_STORAGE: ConstStaticCell<PacketPoolStorage<16>> =
    ConstStaticCell::new(PacketPoolStorage::new());
static GENERAL_POOL: StaticCell<PacketPool<16>> = StaticCell::new();
static RX_STORAGE: ConstStaticCell<PacketPoolStorage<RX>> =
    ConstStaticCell::new(PacketPoolStorage::new());
static RX_POOL: StaticCell<PacketPool<RX>> = StaticCell::new();
static OBSERVATIONS: StaticCell<RefCell<Observations>> = StaticCell::new();

#[inline(never)]
fn observations() -> &'static RefCell<Observations> {
    OBSERVATIONS.init_with(|| RefCell::new(Observations::new(0)))
}

#[inline(never)]
fn reset_observations(observations: &RefCell<Observations>, identifier: u16, lease: Option<Lease>) {
    let mut new = Observations::new(identifier);
    new.lease = lease;
    *observations.borrow_mut() = new;
}
// Admission refusals are retained separately from radio/key evidence.

pub(super) type Running<'a> = impl core::future::Future<Output = ()>;
static RUNNING: ConstStaticCell<oer_memory::StaticPlace<Running<'static>>> =
    ConstStaticCell::new(oer_memory::StaticPlace::new());

#[inline(never)]
pub fn run<'a>(
    address: [u8; 6],
    seed: u64,
    tx: WakingSpscSender<'a, TxMsdu, 2>,
    rx: WakingSpscReceiver<'a, EthernetFrame, RX>,
    link: &'a Cell<bool>,
    attempt: Attempt,
) -> oer_memory::PlacedFuture<'a, Running<'a>> {
    RUNNING
        .take()
        .future(|| running_future(address, seed, tx, rx, link, attempt))
}

#[define_opaque(Running)]
#[inline(never)]
fn running_future<'a>(
    address: [u8; 6],
    seed: u64,
    tx: WakingSpscSender<'a, TxMsdu, 2>,
    rx: WakingSpscReceiver<'a, EthernetFrame, RX>,
    link: &'a Cell<bool>,
    attempt: Attempt,
) -> Running<'a> {
    async move {
        let general = GENERAL_POOL
            .init_with(|| PacketPool::new(GENERAL_STORAGE.take()))
            .allocator();
        let receive = RX_POOL
            .init_with(|| PacketPool::new(RX_STORAGE.take()))
            .allocator();
        let (device, radio) =
            ENDPOINT
                .init_with(Endpoint::new)
                .split(NetworkInterfaceId::new(0), address, receive)
                .unwrap_or_else(|_| crate::fail(crate::fatal::Failure::RootInitialization));
        let (stack, mut runner) = Stack::new(STACK.init_with(StackStorage::new), seed, general);
        runner.set_poll_budget(owned_embassy_net::PollBudget::new(4, 8));
        let iface = stack
            .add_iface_borrowed(DEVICE.init(device))
            .expect("one station IP interface");
        iface
            .set_dhcpv4(Some(DhcpConfig::default()))
            .expect("Ethernet DHCP");
        let socket = RawSocket::new(stack, Some(IpVersion::V4), Some(IpProtocol::Icmp))
            .expect("one station ICMP socket");
        let observations = observations();
        join3(
            bridge(radio, tx, rx, link, &observations),
            runner.run(),
            join3(
                drain(&socket),
                probes(iface, &socket, &observations, link),
                async {
                    let mut previous = None;
                    loop {
                        let current = if link.get() {
                            iface.dhcpv4_lease()
                        } else {
                            None
                        };
                        if current != previous {
                            if let Some(ref lease) = current {
                                observations.borrow_mut().lease =
                                    lease.router.map(|router| Lease {
                                        address: lease.address.address(),
                                        router,
                                        prefix: lease.address.prefix_len(),
                                    });
                                crate::transport::CONSOLE.publish(
                            attempt.request.session_id,
                            attempt.request.request_id,
                            &oer_hil_protocol::network::Ready(
                                oer_hil_protocol::network::NetworkInfo {
                                    network_interface:
                                        oer_hil_protocol::network::WifiNetworkInterface::Station,
                                    address: lease.address.address().octets(),
                                    prefix_length: lease.address.prefix_len(),
                                    gateway: lease.router.map(|router| router.octets()),
                                },
                            ),
                        );
                            }
                            previous = current;
                        }
                        let _ = EmbassyClock.wait_for(Duration::from_millis(10)).await;
                    }
                },
            ),
        )
        .await;
        unreachable!("permanent station IP runners")
    }
}

async fn drain(socket: &RawSocket<'_, Device>) -> ! {
    loop {
        let _ = socket.recv_packet().await;
        yield_now().await;
    }
}

async fn probes(
    iface: Iface<'_, Device>,
    socket: &RawSocket<'_, Device>,
    observations: &RefCell<Observations>,
    link: &Cell<bool>,
) -> ! {
    use core::sync::atomic::Ordering::Relaxed;
    use embassy_futures::select::Either;
    loop {
        let (request, probe) = super::PROBES.receive().await;
        let epoch = super::EPOCH.load(Relaxed);
        let lease = iface.dhcpv4_lease().and_then(|lease| {
            lease.router.map(|router| Lease {
                address: lease.address.address(),
                router,
                prefix: lease.address.prefix_len(),
            })
        });
        reset_observations(observations, probe.identifier, lease);
        super::emit(
            request,
            &oer_hil_protocol::wifi::StationProbeReady {
                epoch,
                identifier: probe.identifier,
                count: probe.count,
            },
        );
        if let Some(lease) = lease {
            let mut next = EmbassyClock.now();
            let deadline = next.saturating_add(Duration::from_secs(u32::from(probe.count) + 15));
            for sequence in 1..=probe.count {
                if !link.get() || super::EPOCH.load(Relaxed) != epoch {
                    break;
                }
                let _ = EmbassyClock.wait_until(next).await;
                let packet = icmp::request(lease.address, lease.router, probe.identifier, sequence);
                if !matches!(
                    select(
                        socket.send(&packet),
                        EmbassyClock.wait_for(Duration::from_millis(250))
                    )
                    .await,
                    Either::First(Ok(()))
                ) {
                    break;
                }
                next = next.saturating_add(Duration::from_secs(1));
            }
            loop {
                let complete = {
                    let o = observations.borrow();
                    o.station.replies() == u32::from(probe.count)
                        && o.peer.replies() == u32::from(probe.count)
                };
                if complete
                    || !link.get()
                    || super::EPOCH.load(Relaxed) != epoch
                    || EmbassyClock.now() >= deadline
                {
                    break;
                }
                let _ = EmbassyClock.wait_for(Duration::from_millis(10)).await;
            }
        }
        let result = {
            let o = observations.borrow();
            StationProbeFinished {
                epoch,
                identifier: probe.identifier,
                sent: u16::try_from(o.station.requests()).expect("bounded probe requests"),
                received: u16::try_from(o.station.replies()).expect("bounded probe replies"),
                peer_requests: u16::try_from(o.peer.requests()).expect("bounded peer requests"),
                peer_replies: u16::try_from(o.peer.replies()).expect("bounded peer replies"),
                malformed: o.malformed,
                queue_drops: o.queue_drops,
            }
        };
        super::emit(request, &result);
    }
}

async fn bridge(
    radio: Runner,
    mut tx: WakingSpscSender<'_, TxMsdu, 2>,
    mut rx: WakingSpscReceiver<'_, EthernetFrame, RX>,
    link: &Cell<bool>,
    observations: &RefCell<Observations>,
) -> ! {
    let mut state = false;
    loop {
        if state != link.get() {
            state = link.get();
            radio.link_controller().set_link_up(state);
        }
        while let Some(frame) = rx.try_receive() {
            let p = frame.parts();
            observations.borrow_mut().observe(
                p.ether_type,
                p.payload,
                false,
                EmbassyClock.now().as_micros(),
            );
            if p.destination == [0xff; 6] && p.ether_type == 0x0806 && valid_arp(p.payload) {
                super::observation::GROUP_ARP.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
            if radio
                .rx_publisher()
                .try_send_parts(p.destination, p.source, p.ether_type, p.payload)
                .is_err()
            {
                super::observation::RX_REFUSED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
        }
        if let Some(packet) = radio.try_receive_tx() {
            let bytes = packet.ethernet();
            if let Some((header, payload)) = bytes.split_first_chunk::<14>() {
                let [d0, d1, d2, d3, d4, d5, s0, s1, s2, s3, s4, s5, e0, e1] = *header;
                if let Ok(frame) = EthernetFrame::from_parts(EthernetFrameParts {
                    destination: [d0, d1, d2, d3, d4, d5],
                    source: [s0, s1, s2, s3, s4, s5],
                    ether_type: u16::from_be_bytes([e0, e1]),
                    payload,
                }) {
                    tx.ready().await.send(TxMsdu {
                        priority: WmmUserPriority::UP0,
                        frame,
                    });
                    observations.borrow_mut().observe(
                        u16::from_be_bytes([e0, e1]),
                        payload,
                        true,
                        EmbassyClock.now().as_micros(),
                    );
                }
            }
            drop(packet);
            yield_now().await;
        } else {
            let _ = select(
                rx.ready(),
                select(
                    radio.wait_tx_publication(),
                    EmbassyClock.wait_for(Duration::from_millis(1)),
                ),
            )
            .await;
        }
    }
}

fn valid_arp(payload: &[u8]) -> bool {
    let Some(bytes) = payload.get(..28) else {
        return false;
    };
    let mut bytes: [u8; 28] = bytes.try_into().expect("checked ARP length");
    ArpPacket::new_checked(&mut bytes).is_ok_and(|packet| {
        packet.hardware_type() == ArpHardware::Ethernet
            && packet.protocol_type() == EthernetProtocol::Ipv4
            && packet.hardware_len() == 6
            && packet.protocol_len() == 4
            && matches!(
                packet.operation(),
                ArpOperation::Request | ArpOperation::Reply
            )
    })
}
