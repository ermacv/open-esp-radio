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
use oer_embassy_net::{NetworkInterfaceId, RxEnqueueError};
use oer_embassy_net::ethernet::{LentCopy, LentSlots};
use oer_embassy_net::ip::{Configuration, Control, Error, Resources, Service};
use oer_esp32s31_wifi::{Network, ReceivedStorage};
use oer_memory::{PlacedFuture, StaticPlace};
use static_cell::{ConstStaticCell, StaticCell};

use crate::fatal::Failure;
use crate::load::counters::Bridged;

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

/// The packets of the frames the station lends in place: as many as its
/// receive buffers the network may hold.
static LENT: LentSlots<ReceivedStorage, { oer_esp32s31_wifi::LENT_RX }> = LentSlots::new();

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
            Some(&LENT),
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

/// The bridge took a frame from the station: the load service counts it.
fn received(_: oer_embassy_net::EthernetFrameView<'_>) {
    crate::load::parts::bridged(Bridged::Received);
}

/// The bridge refused a frame from the station for `reason`.
fn refused(reason: RxEnqueueError) {
    crate::load::parts::bridged(match reason {
        RxEnqueueError::PoolExhausted => Bridged::RefusedPool,
        RxEnqueueError::QueueFull => Bridged::RefusedQueue,
        RxEnqueueError::InvalidLength(_) | RxEnqueueError::LinkDown => Bridged::RefusedOther,
    });
}

/// The bridge copied a frame the station lent in place.
fn copied(_: LentCopy) {
    crate::load::parts::bridged(Bridged::Copied);
}

#[define_opaque(Running)]
fn running_future(
    mut service: Service<Context, RX, TX, Network<'static>, { oer_esp32s31_wifi::LENT_RX }>,
) -> Running {
    async move {
        // Only an access from another context than its executor's fails it.
        if service.run(|_| {}, received, refused, copied, |_| {}).await.is_err() {
            crate::fail(Failure::Network);
        }
    }
}
