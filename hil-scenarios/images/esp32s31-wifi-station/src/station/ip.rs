//! IPv4 by DHCP over the station's neutral Ethernet end. The stack answers
//! ICMP echo once it holds a lease.
use oer_embassy_net::NetworkInterfaceId;
use oer_embassy_net::ip::{Configuration, Control, Error, Resources, Service};
use oer_esp32s31_wifi::Network;
use oer_memory::{PlacedFuture, StaticPlace};
use static_cell::{ConstStaticCell, StaticCell};

use crate::fatal::Failure;

const RX: usize = 8;
const TX: usize = 4;
const GENERAL: usize = 16;
type Context = oer_espressif_executor::ThreadContext<0>;
type IpResources = Resources<Context, RX, TX, GENERAL>;
static IP: StaticCell<IpResources> = StaticCell::new();

pub(crate) type Running = impl core::future::Future<Output = ()>;
static RUNNING: ConstStaticCell<StaticPlace<Running>> = ConstStaticCell::new(StaticPlace::new());

/// Start the one IP stack over `net`: its control, and its service, which
/// runs until the control stops it.
pub(crate) fn start(
    address: [u8; 6],
    seed: u64,
    net: Network<'static>,
) -> Result<(Control<'static>, PlacedFuture<'static, Running>), Error> {
    let resources = IP
        .try_init_with(IpResources::new)
        .unwrap_or_else(|| crate::fail(Failure::Storage));
    let (control, service) = resources
        .start(
            net,
            NetworkInterfaceId::new(0),
            address,
            seed,
            Configuration::Dhcpv4,
        )
        .map_err(|refused| refused.reason)?;
    let place = RUNNING
        .try_take()
        .unwrap_or_else(|| crate::fail(Failure::Storage));
    Ok((control, place.future(|| running_future(service))))
}

#[define_opaque(Running)]
fn running_future(mut service: Service<Context, RX, TX, Network<'static>>) -> Running {
    async move {
        // Only an access from another context than its executor's fails it.
        if service.run(|_| {}, |_| {}, |_| {}, |_| {}).await.is_err() {
            crate::fail(Failure::Network);
        }
    }
}
