//! P1 DHCP/ICMP stack over the existing owned Ethernet adapter.
use super::Attempt;
use core::sync::atomic::Ordering::Relaxed;
use oer_embassy_net::ip::{Configuration, Ipv4Status, Resources};
use oer_esp32s31_wifi::Network;
use owned_embassy_net::wire::{ArpHardware, ArpOperation, ArpPacket, EthernetProtocol};
use static_cell::{ConstStaticCell, StaticCell};

pub const RX: usize = 8;
const TX: usize = 4;
type IpResources = Resources<oer_espressif_executor::ThreadContext<0>, RX, TX, 16>;
static IP: StaticCell<IpResources> = StaticCell::new();
pub(super) type Running<'a> = impl core::future::Future<Output = ()>;
static RUNNING: ConstStaticCell<oer_memory::StaticPlace<Running<'static>>> =
    ConstStaticCell::new(oer_memory::StaticPlace::new());

#[inline(never)]
pub fn run<'a>(
    address: [u8; 6],
    seed: u64,
    net: Network<'a>,
    attempt: Attempt,
) -> oer_memory::PlacedFuture<'a, Running<'a>> {
    RUNNING
        .take()
        .future(|| running_future(address, seed, net, attempt))
}

#[define_opaque(Running)]
#[inline(never)]
fn running_future<'a>(
    address: [u8; 6],
    seed: u64,
    net: Network<'a>,
    attempt: Attempt,
) -> Running<'a> {
    async move {
        let resources = IP
            .try_init_with(IpResources::new)
            .unwrap_or_else(|| crate::fail(crate::fatal::Failure::RootInitialization));
        let (_control, mut service) = resources
            .start(
                net,
                oer_embassy_net::NetworkInterfaceId::new(0),
                address,
                seed,
                Configuration::Dhcpv4,
            )
            .unwrap_or_else(|_| crate::fail(crate::fatal::Failure::RootInitialization));
        match service
            .run(
                |event| {
                    if let Ipv4Status::Lease(config) = event.current.ipv4 {
                        crate::transport::CONSOLE.publish(
                            attempt.request.session_id,
                            attempt.request.request_id,
                            &oer_network_control::Ready(oer_network_control::NetworkInfo {
                                network_interface:
                                    oer_network_control::WifiNetworkInterface::Station,
                                address: config.address,
                                prefix_length: config.prefix_length,
                                gateway: config.gateway,
                            }),
                        );
                    }
                },
                |p| {
                    if p.destination == [0xff; 6] && p.ether_type == 0x0806 && valid_arp(p.payload)
                    {
                        super::observation::GROUP_ARP.fetch_add(1, Relaxed);
                    }
                },
                |_| {
                    super::observation::RX_REFUSED.fetch_add(1, Relaxed);
                },
                |_| {},
            )
            .await
        {
            Ok(()) => crate::fail(crate::fatal::Failure::RootInitialization),
            Err(_) => crate::fail(crate::fatal::Failure::NetworkContextAccessRefused),
        }
    }
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
