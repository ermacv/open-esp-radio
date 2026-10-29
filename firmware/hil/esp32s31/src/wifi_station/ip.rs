//! P1 DHCP/ICMP stack over the existing owned Ethernet adapter.
use super::Attempt;
use oer_embassy_net::ip::{Configuration, Ipv4Status, Resources};
use oer_esp32s31_wifi::Network;
#[cfg(oer_trace)]
use owned_embassy_net::wire::{ArpHardware, ArpOperation, ArpPacket, EthernetProtocol};
use static_cell::{ConstStaticCell, StaticCell};

pub const RX: usize = 8;
const TX: usize = 4;
type IpResources = Resources<oer_espressif_executor::ThreadContext<0>, RX, TX, 16>;
pub(super) type Device =
    oer_embassy_net::OwnedNetworkDevice<'static, oer_espressif_executor::ThreadContext<0>, RX, TX>;
pub(super) type Stack = owned_embassy_net::Stack<'static, Device>;
static IP: StaticCell<IpResources> = StaticCell::new();
pub(super) type Running = impl core::future::Future<Output = ()>;
static RUNNING: ConstStaticCell<oer_memory::StaticPlace<Running>> =
    ConstStaticCell::new(oer_memory::StaticPlace::new());

pub fn start(
    address: [u8; 6],
    seed: u64,
    net: Network<'static>,
    attempt: Attempt,
    observations: &'static super::ObservationStorage,
) -> (
    oer_embassy_net::ip::Control<'static>,
    oer_memory::PlacedFuture<'static, Running>,
) {
    let resources = IP
        .try_init_with(IpResources::new)
        .unwrap_or_else(|| crate::fail(crate::fatal::Failure::RootInitialization));
    let (control, service) = resources
        .start(
            net,
            oer_embassy_net::NetworkInterfaceId::new(0),
            address,
            seed,
            Configuration::Dhcpv4,
        )
        .unwrap_or_else(|_| crate::fail(crate::fatal::Failure::RootInitialization));

    let work = RUNNING
        .take()
        .future(|| running_future(service, attempt, observations));
    (control, work)
}
#[define_opaque(Running)]
fn running_future(
    mut service: oer_embassy_net::ip::Service<
        oer_espressif_executor::ThreadContext<0>,
        RX,
        TX,
        Network<'static>,
    >,
    attempt: Attempt,
    observations: &'static super::ObservationStorage,
) -> Running {
    async move {
        let stack = service.stack();
        let network = service.run(
            |event| {
                if let Ipv4Status::Lease(config) = event.current.ipv4 {
                    crate::transport::CONSOLE.publish(
                        attempt.request.session_id,
                        attempt.request.request_id,
                        &oer_network_control::Ready(oer_network_control::NetworkInfo {
                            network_interface: oer_network_control::WifiNetworkInterface::Station,
                            address: config.address,
                            prefix_length: config.prefix_length,
                            gateway: config.gateway,
                        }),
                    );
                }
            },
            |p| {
                #[cfg(oer_trace)]
                if p.destination == [0xff; 6] && p.ether_type == 0x0806 && valid_arp(p.payload) {
                    oer_trace::observe!(observations, oer_ieee80211_control::collector::NetworkArp);
                }
                let _ = p;
            },
            |_| {
                oer_trace::observe!(
                    observations,
                    oer_ieee80211_control::collector::NetworkRefused
                );
            },
            |_| {},
        );
        match embassy_futures::select::select(network, super::udp::run(stack)).await {
            embassy_futures::select::Either::Second(never) => never,
            embassy_futures::select::Either::First(Ok(())) => (),
            embassy_futures::select::Either::First(Err(_)) => {
                crate::fail(crate::fatal::Failure::NetworkContextAccessRefused)
            }
        }
    }
}

#[cfg(oer_trace)]
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
