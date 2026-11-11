//! The IP service of xarxa over the station: the station's transmit buffers
//! as the stack's packets, its pools and its task. The `network` feature puts
//! the new stack in this place.

use super::*;
use oer_portable_embassy_net::{
    NetworkInterfaceId,
    ethernet::{LentSlots, ParkedTx, TxSlots, TxWriteRefused},
    ip::{self, Configuration, Resources, Service},
};
use oer_protocols_ieee80211_runtime::station::{TxRefused, TxSupply};

pub use embassy_net;

/// The IP stack's packet owner a transmission hands the station.
pub(super) type TxPacket = oer_portable_embassy_net::OwnedNetworkTxFrame;

/// The packet slots over the station's transmit buffers, but the spare the
/// network keeps for copies: the IP stack's general allocator.
static TX_SLOTS: TxSlots<wifi::TxSlot, { wifi::NETWORK_BUFFERS - 1 }> = TxSlots::new();

/// The network end's transmit memory: the station's transmit buffers parked
/// in [`TX_SLOTS`], where the IP stack writes its packets, and the spare
/// that a packet from elsewhere is copied into. A packet goes to the
/// station in the buffer it was written in.
/// The station's transmit end the IP service takes: it parks or copies
/// each packet into a transmit buffer.
pub(super) type TxEnd = wifi::SupplyEnd<WifiTx>;

pub struct WifiTx {
    publisher: wifi::TxPublisher,
    parked: ParkedTx<wifi::TxSlot, { wifi::NETWORK_BUFFERS - 1 }>,
}

impl wifi::NetworkTxSupply for WifiTx {
    type Packet = TxPacket;
    fn new(tx: wifi::NetworkTx) -> Self {
        let (publisher, slots) = tx.into_parts();
        let mut parked = ParkedTx::new(&TX_SLOTS);
        for slot in slots {
            parked.give(slot);
        }
        Self { publisher, parked }
    }
}

impl TxSupply<TxPacket> for WifiTx {
    type Memory = wifi::TxMemory;
    fn available(&self) -> bool {
        self.parked.available()
    }
    fn write(&mut self, frame: TxPacket) -> Result<wifi::TxMemory, TxRefused<TxPacket>> {
        let written = match self.parked.write(frame) {
            Ok(written) => written,
            Err(TxWriteRefused::Kept(refused)) => return Err(TxRefused::Kept(refused)),
            Err(TxWriteRefused::Lost) => return Err(TxRefused::Lost),
        };
        self.publisher
            .publish(written.storage, written.start, written.len)
            .map_err(|refused| {
                // A packet stays inside its buffer: a frame past it ends
                // here, and the buffer is free again.
                if let wifi::TxPublishRefused::Range(slot) = refused {
                    self.parked.give(slot);
                }
                TxRefused::Lost
            })
    }
    /// xarxa's egress keeps no airtime account: the end is not read.
    fn give(
        &mut self,
        memory: wifi::TxMemory,
        _end: oer_protocols_ieee80211_contracts_port::TxEnd,
    ) {
        self.parked.give(self.publisher.slot(memory));
    }
}

/// The default packet pool of the IP service: receive slots, each holding
/// one 1514-byte Ethernet frame. The stack's general packets are the
/// station's transmit buffers ([`TX_SLOTS`]).
pub const RX: usize = 8;

type IpResources<const RX: usize> = Resources<Context, RX, Transmit>;
type IpService<const RX: usize> =
    Service<Context, RX, Transmit, wifi::Network<'static, Notifies>, { wifi::LENT_RX }>;

/// The IP service's packet pool: `RX` receive slots, each one 1514-byte
/// frame. A static of this type is the storage [`Net::start_with`] takes
/// once; [`Net::start`] takes the library's own of [`RX`].
pub struct Pools<const RX: usize> {
    resources: StaticCell<IpResources<RX>>,
    control: StaticCell<ip::Control<'static>>,
}

impl<const RX: usize> Pools<RX> {
    pub const fn new() -> Self {
        Self {
            resources: StaticCell::new(),
            control: StaticCell::new(),
        }
    }
}

impl<const RX: usize> Default for Pools<RX> {
    fn default() -> Self {
        Self::new()
    }
}

static POOLS: Pools<RX> = Pools::new();

/// What the IP service runs: the service, the station's stop request, and
/// the service's control to stop it with, at the image's pool sizes.
#[doc(hidden)]
pub struct IpStart<const RX: usize> {
    service: IpService<RX>,
    stopper: wifi::NetworkStopper<'static, Notifies>,
    control: &'static ip::Control<'static>,
}

impl<const RX: usize> crate::NetTask for IpStart<RX> {
    fn serve(self) -> impl core::future::Future<Output = ()> {
        serve_ip(self)
    }
}

/// The packets of the frames the station lends in place: as many as its
/// receive buffers the network may hold.
static LENT: LentSlots<wifi::ReceivedStorage<Notifies>, { wifi::LENT_RX }> = LentSlots::new();

/// The IP stack's device, which sockets name, over a pool of `RX`.
pub type IpDevice<const RX: usize = { self::RX }> =
    oer_portable_embassy_net::OwnedNetworkDevice<'static, Context, RX, Transmit>;

/// The IP stack sockets open on.
pub type Stack<const RX: usize = { self::RX }> = embassy_net::Stack<'static, IpDevice<RX>>;

/// How the stack takes its IPv4 configuration.
pub use oer_portable_embassy_net::ip::Configuration as NetConfig;

/// The IP service over the station's device, with a pool of `RX` receive
/// slots; the default is the library's.
pub struct Net<const RX: usize = { self::RX }> {
    control: &'static ip::Control<'static>,
    stack: Stack<RX>,
}

impl Net {
    /// Start the IP service over `device` with `configuration`
    /// (`NetConfig::Dhcpv4` or a static address) and the library's pools,
    /// in the group; it returns with the service, whose task runs
    /// once the executor does.
    ///
    /// # Errors
    /// The service started already or refused its configuration.
    pub fn start(
        device: Device,
        configuration: Configuration,
        boot: GroupBoot,
    ) -> Result<(Self, GroupBoot<IpStart<RX>>), Error> {
        Self::start_with(device, &POOLS, configuration, boot)
    }
}

impl<const RX: usize> Net<RX> {
    /// As [`Net::start`], with the image's own `pools`.
    ///
    /// # Errors
    /// As [`Net::start`]; the pools were taken already.
    pub fn start_with(
        mut device: Device,
        pools: &'static Pools<RX>,
        configuration: Configuration,
        boot: GroupBoot,
    ) -> Result<(Self, GroupBoot<IpStart<RX>>), Error> {
        let (network, transmit, stopper) = device.network.take().ok_or(Error::AlreadyStarted)?;
        let resources = pools
            .resources
            .try_uninit()
            .map(|place| place.write(const { IpResources::<RX>::new() }))
            .ok_or(Error::AlreadyStarted)?;
        let (control, service) = resources
            .start(
                network,
                transmit,
                Some(&LENT),
                TX_SLOTS.allocator(),
                NetworkInterfaceId::new(0),
                device.address,
                device.seed,
                configuration,
            )
            .map_err(Error::Net)?;
        let control: &'static ip::Control<'static> =
            pools.control.try_init(control).ok_or(Error::Storage)?;
        let stack = service.stack();
        Ok((
            Self { control, stack },
            boot.with_net(IpStart {
                service,
                stopper,
                control,
            }),
        ))
    }

    /// The stack sockets open on.
    pub fn stack(&self) -> Stack<RX> {
        self.stack
    }

    /// The service's link, IPv4 and lifecycle now.
    pub fn status(&self) -> Status {
        self.control.status()
    }

    /// Wait for an IPv4 configuration over an up link; across a link loss
    /// the wait goes on to the next lease.
    ///
    /// # Errors
    /// The service stopped or failed, or too many tasks wait at once.
    pub async fn wait_ipv4(&self) -> Result<Ipv4Configuration, NetError> {
        crate::net::wait_ipv4(&WATCHERS, || self.control.status()).await
    }

    /// Wait until the link that is up now goes down or is replaced.
    ///
    /// # Errors
    /// As [`Self::wait_ipv4`].
    pub async fn wait_link_down(&self) -> Result<(), NetError> {
        crate::net::wait_link_down(&WATCHERS, || self.control.status()).await
    }
}

#[cfg(feature = "observe")]
use crate::observed::{bridge_copied, bridge_received, bridge_refused};
/// The network bridge's frames: counted with `observe`.
#[cfg(not(feature = "observe"))]
fn bridge_received(_: oer_portable_embassy_net::EthernetFrameView<'_>) {}
#[cfg(not(feature = "observe"))]
fn bridge_refused(_: oer_portable_embassy_net::RxEnqueueError) {}
#[cfg(not(feature = "observe"))]
fn bridge_copied(_: oer_portable_embassy_net::ethernet::LentCopy) {}

/// The IP service's task: every status change wakes the waits. Once the
/// station asks its network to stop, the service stops and returns every
/// receive buffer it holds, then the station hears that it stopped.
async fn serve_ip<const RX: usize>(start: IpStart<RX>) {
    let IpStart {
        mut service,
        stopper,
        control,
    } = start;
    let result = {
        let mut run = core::pin::pin!(service.run(
            |changed| publish(changed.current),
            bridge_received,
            bridge_refused,
            bridge_copied,
        ));
        match select(run.as_mut(), stopper.requested()).await {
            Either::First(result) => result,
            Either::Second(()) => {
                control.stop();
                run.await
            }
        }
    };
    publish(control.status());
    if result.is_err() {
        crate::fail(oer_portable_diagnostics::name!("net-context"; 32));
    }
    stopper.stopped();
}
