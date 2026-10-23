//! P1 DHCP/ICMP stack over the existing owned Ethernet adapter.
use super::Attempt;
use core::cell::Cell;
use embassy_futures::{join::join3, select::select, yield_now};
use oer_embassy_net::{
    NetworkInterfaceId, NoopRawMutex, OwnedEndpointResources, OwnedNetworkDevice,
    OwnedNetworkRunner,
};
use oer_embassy_time::EmbassyClock;
use oer_ieee80211_runtime::station_service::*;
use oer_ieee80211_wire::{data::EthernetFrameParts, qos::WmmUserPriority};
use oer_time::{Duration, Timer};
use owned_embassy_net::{Stack, StackStorage, iface::dhcpv4::DhcpConfig};
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
// Admission refusals are retained separately from radio/key evidence.
static RX_REFUSED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

pub(super) type Running<'a> = impl core::future::Future<Output = ()>;
static RUNNING: ConstStaticCell<oer_memory::StaticPlace<Running<'static>>> =
    ConstStaticCell::new(oer_memory::StaticPlace::new());

pub fn run<'a>(
    address: [u8; 6],
    seed: u64,
    tx: MsduSender<'a, TxMsdu, 2>,
    rx: MsduReceiver<'a, EthernetFrame, RX>,
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
    tx: MsduSender<'a, TxMsdu, 2>,
    rx: MsduReceiver<'a, EthernetFrame, RX>,
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
                .split(NetworkInterfaceId::new(0), address, receive);
        let (stack, mut runner) = Stack::new(STACK.init_with(StackStorage::new), seed, general);
        runner.set_poll_budget(owned_embassy_net::PollBudget::new(4, 8));
        let iface = stack
            .add_iface_borrowed(DEVICE.init(device))
            .expect("one station IP interface");
        iface
            .set_dhcpv4(Some(DhcpConfig::default()))
            .expect("Ethernet DHCP");
        join3(bridge(radio, tx, rx, link), runner.run(), async {
            let mut previous = None;
            loop {
                let current = if link.get() {
                    iface.dhcpv4_lease()
                } else {
                    None
                };
                if current != previous {
                    if let Some(ref lease) = current {
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
        })
        .await;
        unreachable!("permanent station IP runners")
    }
}

async fn bridge(
    radio: Runner,
    mut tx: MsduSender<'_, TxMsdu, 2>,
    mut rx: MsduReceiver<'_, EthernetFrame, RX>,
    link: &Cell<bool>,
) -> ! {
    let mut state = false;
    loop {
        if state != link.get() {
            state = link.get();
            radio.link_controller().set_link_up(state);
        }
        while let Some(frame) = rx.try_receive() {
            let p = frame.parts();
            if radio
                .rx_publisher()
                .try_send_parts(p.destination, p.source, p.ether_type, p.payload)
                .is_err()
            {
                RX_REFUSED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
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
                        priority: WmmUserPriority::new(0).expect("best effort priority"),
                        frame,
                    });
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
