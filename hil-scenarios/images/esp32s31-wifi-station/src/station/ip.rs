//! IPv4 by DHCP over the station's neutral Ethernet end. The stack answers
//! ICMP echo once it holds a lease.
//!
//! The pools are sized for the load service's UDP load of about 65 Mbit/s
//! in 1400-byte datagrams, some 5 800 datagrams a second (one each 172 µs)
//! each way: the bridge drains every frame the station delivers into the
//! receive pool, so its 32 slots hold an aggregate burst of some 5.5 ms
//! while the stack and the load's socket (a queue of 16) take them; the 16
//! transmit credits and the 32 general slots hold some 2.7 ms of transmit
//! batches beside the stack's own packets. Each slot holds one 1514-byte
//! Ethernet frame.
use oer_embassy_net::NetworkInterfaceId;
use oer_embassy_net::ip::{Configuration, Control, Error, Resources, Service};
use oer_esp32s31_wifi::Network;
use oer_memory::{PlacedFuture, StaticPlace};
use static_cell::{ConstStaticCell, StaticCell};

use crate::fatal::Failure;

const RX: usize = 32;
const TX: usize = 16;
const GENERAL: usize = 32;
type Context = oer_espressif_executor::ThreadContext<0>;
type IpResources = Resources<Context, RX, TX, GENERAL>;
/// The stack's device.
pub(crate) type Device = oer_embassy_net::OwnedNetworkDevice<'static, Context, RX, TX>;
/// The stack's one interface, through which the load service opens its
/// socket and reads the lease.
pub(crate) type Interface = embassy_net::iface::Iface<'static, Device>;
static IP: StaticCell<IpResources> = StaticCell::new();

pub(crate) type Running = impl core::future::Future<Output = ()>;
static RUNNING: ConstStaticCell<StaticPlace<Running>> = ConstStaticCell::new(StaticPlace::new());

/// Start the one IP stack over `net`: its control, its interface, and its
/// service, which runs until the control stops it.
pub(crate) fn start(
    address: [u8; 6],
    seed: u64,
    net: Network<'static>,
) -> Result<(Control<'static>, Interface, PlacedFuture<'static, Running>), Error> {
    // Written from a constant, so the pools are never a value on the stack.
    let resources = IP
        .try_uninit()
        .map(|place| place.write(const { IpResources::new() }))
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
    let interface = service.interface();
    Ok((control, interface, place.future(|| running_future(service))))
}

#[define_opaque(Running)]
fn running_future(mut service: Service<Context, RX, TX, Network<'static>>) -> Running {
    async move {
        // Only an access from another context than its executor's fails it.
        if service.run(
            |_| {},
            |_| crate::load::parts::mark(crate::load::parts::Part::NetRx),
            |reason| {
                use crate::load::parts::{Part, mark};
                mark(Part::NetRefused);
                match reason {
                    oer_embassy_net::RxEnqueueError::PoolExhausted => mark(Part::NetPool),
                    oer_embassy_net::RxEnqueueError::QueueFull => mark(Part::NetQueue),
                    _ => {}
                }
            },
            |_| {},
        ).await.is_err() {
            crate::fail(Failure::Network);
        }
    }
}
