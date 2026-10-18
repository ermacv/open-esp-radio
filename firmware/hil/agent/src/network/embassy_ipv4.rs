//! Per-interface HIL IPv4 policy over the owned Embassy interface API.
use embassy_net::Stack;
use embassy_net::iface;
use embassy_net::wire::{IpAddr, IpCidr, Ipv4Addr, Ipv4Cidr};
use oer_hil_protocol::{
    network::NetworkInfo, wifi::NetworkIpv4Configuration, wifi::WifiNetworkInterface,
};

#[derive(Clone, Copy)]
pub struct Iface<'a>(pub iface::Iface<'a>);

impl<'a> Iface<'a> {
    pub fn stack(self) -> Stack<'a> {
        self.0.stack()
    }
    pub async fn wait_config_v4_up(self) {
        self.0.wait_config_v4_up().await;
    }
    pub async fn wait_config_v4_down(self) {
        self.0.wait_config_v4_down().await;
    }
}

/// Replace the interface's IPv4 addresses, gateway and DHCPv4 client.
pub fn configure(iface: Iface<'_>, config: Option<NetworkIpv4Configuration>) {
    let iface = iface.0;
    iface
        .set_dhcpv4(None)
        .expect("the HIL Wi-Fi interface is Ethernet");
    iface.set_ip_addrs([]).expect("an empty address list fits");
    iface
        .stack()
        .routes()
        .retain(|route| route.iface != iface.handle());
    match config {
        None => {}
        Some(NetworkIpv4Configuration::Dhcp) => iface
            .set_dhcpv4(Some(Default::default()))
            .expect("the HIL Wi-Fi interface is Ethernet"),
        Some(NetworkIpv4Configuration::Static {
            address,
            prefix_length,
            gateway,
        }) => {
            iface
                .add_ip_addr(IpCidr::V4(Ipv4Cidr::new(
                    Ipv4Addr::from(address),
                    prefix_length,
                )))
                .expect("one HIL IPv4 address fits");
            if let Some(gateway) = gateway {
                iface
                    .stack()
                    .routes()
                    .add_default_ipv4_route(Ipv4Addr::from(gateway), iface.handle())
                    .expect("one HIL gateway fits");
            }
        }
    }
}

pub fn info(iface: Iface<'_>, network_interface: WifiNetworkInterface) -> Option<NetworkInfo> {
    let iface = iface.0;
    let address = iface.ip_addrs().iter().find_map(|addr| match addr.cidr {
        IpCidr::V4(cidr) => Some(cidr),
        #[allow(unreachable_patterns)]
        _ => None,
    })?;
    let gateway = iface
        .stack()
        .routes()
        .iter()
        .find(|route| route.iface == iface.handle() && route.is_ipv4_gateway())
        .and_then(|route| match route.via_router {
            IpAddr::V4(address) => Some(address.octets()),
            #[allow(unreachable_patterns)]
            _ => None,
        });
    Some(NetworkInfo {
        network_interface,
        address: address.address().octets(),
        prefix_length: address.prefix_len(),
        gateway,
    })
}
