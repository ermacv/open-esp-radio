//! P1 DHCP/ICMP stack over the existing owned Ethernet adapter.
use super::Attempt;
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};
use embassy_futures::{join::join3, select::select, yield_now};
use oer_embassy_net::{
    NetworkInterfaceId, OwnedEndpointResources, OwnedNetworkDevice, OwnedNetworkRunner,
};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_wifi::Network;
use oer_firmware_apps_w3::{icmp, observation::Observations, report::Lease};
use oer_ieee80211_control::StationProbeFinished;
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
type Endpoint = OwnedEndpointResources<oer_espressif_executor::ThreadContext<0>, RX, TX>;
type Device = OwnedNetworkDevice<'static, oer_espressif_executor::ThreadContext<0>, RX, TX>;
type Runner = OwnedNetworkRunner<'static, oer_espressif_executor::ThreadContext<0>, RX, TX>;
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
    net: Network<'a>,
    link: &'a AtomicBool,
    attempt: Attempt,
) -> oer_memory::PlacedFuture<'a, Running<'a>> {
    RUNNING
        .take()
        .future(|| running_future(address, seed, net, link, attempt))
}

#[define_opaque(Running)]
#[inline(never)]
fn running_future<'a>(
    address: [u8; 6],
    seed: u64,
    net: Network<'a>,
    link: &'a AtomicBool,
    attempt: Attempt,
) -> Running<'a> {
    async move {
        let general = GENERAL_POOL
            .init_with(|| PacketPool::new(GENERAL_STORAGE.take()))
            .allocator();
        let receive = RX_POOL
            .init_with(|| PacketPool::new(RX_STORAGE.take()))
            .allocator();
        let (device, radio) = ENDPOINT
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
            async {
                match bridge(radio, net, observations).await {
                    Ok(never) => match never {},
                    Err(_) => crate::fail(crate::fatal::Failure::NetworkContextAccessRefused),
                }
            },
            runner.run(),
            join3(
                drain(&socket),
                probes(iface, &socket, &observations, link),
                async {
                    let mut previous = None;
                    loop {
                        let current = if link.load(Relaxed) {
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
                                    &oer_network_control::Ready(oer_network_control::NetworkInfo {
                                        network_interface:
                                            oer_network_control::WifiNetworkInterface::Station,
                                        address: lease.address.address().octets(),
                                        prefix_length: lease.address.prefix_len(),
                                        gateway: lease.router.map(|router| router.octets()),
                                    }),
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
    link: &AtomicBool,
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
            &oer_ieee80211_control::StationProbeReady {
                epoch,
                identifier: probe.identifier,
                count: probe.count,
            },
        );
        if let Some(lease) = lease {
            let mut next = EmbassyClock.now();
            let deadline = next.saturating_add(Duration::from_secs(u32::from(probe.count) + 15));
            for sequence in 1..=probe.count {
                if !link.load(Relaxed) || super::EPOCH.load(Relaxed) != epoch {
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
                    || !link.load(Relaxed)
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
    net: Network<'_>,
    observations: &RefCell<Observations>,
) -> Result<core::convert::Infallible, oer_embassy_net::AccessRefused> {
    let mut bridge = oer_embassy_net::ethernet::EthernetBridge::new(radio, net);
    loop {
        bridge.synchronize_link();
        bridge.drain_receive(
            |p| {
                observations.borrow_mut().observe(
                    p.ether_type,
                    p.payload,
                    false,
                    EmbassyClock.now().as_micros(),
                );
                if p.destination == [0xff; 6] && p.ether_type == 0x0806 && valid_arp(p.payload) {
                    super::observation::GROUP_ARP
                        .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                }
            },
            |_| {
                super::observation::RX_REFUSED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            },
        );
        if bridge
            .transmit_one(|p| {
                observations.borrow_mut().observe(
                    p.ether_type,
                    p.payload,
                    true,
                    EmbassyClock.now().as_micros(),
                );
            })
            .await?
        {
            yield_now().await;
        } else {
            bridge
                .wait(async {
                    let _timeout = EmbassyClock.wait_for(Duration::from_millis(1)).await;
                })
                .await?;
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
