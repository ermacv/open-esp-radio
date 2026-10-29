//! P1 DHCP/ICMP stack over the existing owned Ethernet adapter.
use super::Attempt;
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};
use embassy_futures::join::join;
use oer_embassy_net::{
    NetworkInterfaceId, OwnedEndpointResources, OwnedNetworkDevice, OwnedNetworkRunner,
};
use oer_esp32s31_wifi::Network;
use owned_embassy_net::{
    Stack, StackStorage,
    iface::dhcpv4::DhcpConfig,
    wire::{ArpHardware, ArpOperation, ArpPacket, EthernetProtocol},
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
        use core::future::Future;
        let mut running = core::pin::pin!(runner.run());
        let mut previous = None;
        // Inspect status after each event-driven IP poll, including a renewed
        // lease that remains configured. No periodic lease sampler is needed.
        let stack_task = core::future::poll_fn(|cx| {
            let result = running.as_mut().poll(cx);
            let current = if link.load(Relaxed) {
                iface.dhcpv4_lease()
            } else {
                None
            };
            if current != previous {
                if let Some(ref lease) = current {
                    crate::transport::CONSOLE.publish(
                        attempt.request.session_id,
                        attempt.request.request_id,
                        &oer_network_control::Ready(oer_network_control::NetworkInfo {
                            network_interface: oer_network_control::WifiNetworkInterface::Station,
                            address: lease.address.address().octets(),
                            prefix_length: lease.address.prefix_len(),
                            gateway: lease.router.map(|router| router.octets()),
                        }),
                    );
                }
                previous = current;
            }
            result
        });
        join(
            async {
                match bridge(radio, net).await {
                    Ok(never) => match never {},
                    Err(_) => crate::fail(crate::fatal::Failure::NetworkContextAccessRefused),
                }
            },
            stack_task,
        )
        .await;
        crate::fail(crate::fatal::Failure::RootInitialization)
    }
}

async fn bridge(
    radio: Runner,
    net: Network<'_>,
) -> Result<core::convert::Infallible, oer_embassy_net::AccessRefused> {
    let mut bridge = oer_embassy_net::ethernet::EthernetBridge::new(radio, net);
    core::future::poll_fn(|cx| {
        bridge.poll(
            cx,
            |p| {
                if p.destination == [0xff; 6] && p.ether_type == 0x0806 && valid_arp(p.payload) {
                    super::observation::GROUP_ARP.fetch_add(1, Relaxed);
                }
            },
            |_| {
                super::observation::RX_REFUSED.fetch_add(1, Relaxed);
            },
            |_| {},
        )
    })
    .await
}

fn valid_arp(payload: &[u8]) -> bool {
    let Some(mut bytes) = payload.first_chunk::<28>().copied() else {
        return false;
    };
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
