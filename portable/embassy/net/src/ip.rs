//! One permanent IP stack over an affine [`NetDevice`](oer_protocols_network_port::NetDevice).
//!
//! Place [`Resources`] once, then call [`Resources::start`]. Reassociation
//! changes the link epoch and invalidates DHCP, without reconstructing pools,
//! the stack, sockets or the Ethernet bridge. Socket applications use the pinned
//! stack returned by [`Service::stack`]; this module defines no socket API.

use core::{cell::Cell, future::Future, task::Poll};
use oer_portable_memory::ExecutionContext;
use oer_portable_sync::PriorityWaker;
use oer_protocols_network_port::{
    EthernetFrameView, LinkSnapshot, LinkState, NetDevice, NetTransmit, NetworkInterfaceId,
};
use owned_embassy_net::{
    ClaimedPool, Stack, StackStorage,
    iface::{Iface, dhcpv4::DhcpConfig},
    wire::{IpCidr, Ipv4Addr, Ipv4Cidr},
};
use xarxa_driver::{PacketBufAllocator, PacketPool, PacketPoolStorage};

use crate::{
    OwnedEndpointResources, OwnedNetworkDevice, OwnedNetworkTxFrame, ethernet::EthernetBridge,
};

/// IPv4 values independent of the selected socket stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ipv4Configuration {
    pub address: [u8; 4],
    pub prefix_length: u8,
    pub gateway: Option<[u8; 4]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Configuration {
    Dhcpv4,
    Static(Ipv4Configuration),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ipv4Status {
    Unconfigured,
    Lease(Ipv4Configuration),
    Static(Ipv4Configuration),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidPrefixLength(u8),
    EndpointAlreadySplit,
    RxWaiterUnavailable,
    /// Another stack waits on the general allocator already.
    GeneralWaiterUnavailable,
    InterfaceTableFull,
    UnsupportedMedium,
    HardwareAddressMismatch,
    AddressNotUnicast,
    AddressTableFull,
    GatewayNotUnicast,
    RouteTableFull,
    DhcpMediumMismatch,
}

impl From<crate::EndpointSplitError> for Error {
    fn from(error: crate::EndpointSplitError) -> Self {
        match error {
            crate::EndpointSplitError::AlreadySplit => Self::EndpointAlreadySplit,
            crate::EndpointSplitError::RxWaiterUnavailable => Self::RxWaiterUnavailable,
        }
    }
}

impl From<owned_embassy_net::iface::AddIfaceError> for Error {
    fn from(error: owned_embassy_net::iface::AddIfaceError) -> Self {
        use owned_embassy_net::iface::AddIfaceError;
        match error {
            AddIfaceError::Full => Self::InterfaceTableFull,
            AddIfaceError::UnsupportedMedium => Self::UnsupportedMedium,
            AddIfaceError::HardwareAddrMismatch => Self::HardwareAddressMismatch,
        }
    }
}

impl From<owned_embassy_net::iface::AddrError> for Error {
    fn from(error: owned_embassy_net::iface::AddrError) -> Self {
        use owned_embassy_net::iface::AddrError;
        match error {
            AddrError::NotUnicast => Self::AddressNotUnicast,
            AddrError::Full => Self::AddressTableFull,
        }
    }
}

impl From<owned_embassy_net::route::RouteError> for Error {
    fn from(error: owned_embassy_net::route::RouteError) -> Self {
        use owned_embassy_net::route::RouteError;
        match error {
            RouteError::NotUnicast => Self::GatewayNotUnicast,
            RouteError::Full => Self::RouteTableFull,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lifecycle {
    Running,
    Stopped,
    Failed(Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Status {
    pub link: LinkSnapshot,
    pub ipv4: Ipv4Status,
    pub lifecycle: Lifecycle,
}

impl Status {
    const INITIAL: Self = Self {
        link: LinkSnapshot::DOWN,
        ipv4: Ipv4Status::Unconfigured,
        lifecycle: Lifecycle::Running,
    };
}

/// Synchronous delivery of each observed change in the service's task; no queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatusChanged {
    pub previous: Status,
    pub current: Status,
}

struct State {
    status: Cell<Status>,
    stop: Cell<bool>,
    runner: PriorityWaker,
}

impl State {
    const fn new() -> Self {
        Self {
            status: Cell::new(Status::INITIAL),
            stop: Cell::new(false),
            runner: PriorityWaker::new(),
        }
    }

    fn publish(&self, current: Status, events: &mut impl FnMut(StatusChanged)) {
        let previous = self.status.replace(current);
        if current != previous {
            events(StatusChanged { previous, current });
        }
    }
}

/// Local control capability. Keep it on the same executor/hart as the service.
/// Stop is independent of packet/command queue capacity and is idempotent.
pub struct Control<'a> {
    state: &'a State,
}

impl Control<'_> {
    pub fn status(&self) -> Status {
        self.state.status.get()
    }

    pub fn stop(&self) {
        self.state.stop.set(true);
        self.state.runner.wake();
    }
}

/// Final storage for one stack and its private receive pool. Consuming the
/// exclusive static borrow in `start` prevents a second start or pool waiter.
///
/// Memory placement belongs to the image. There are RX receive packet/queue
/// slots and the device's transmit half `T`; the general packets come from
/// the allocator `start` takes, such as the device's
/// [`TxSlots`](crate::ethernet::TxSlots).
///
/// ```compile_fail,E0499
/// use oer_portable_embassy_net::{NetworkInterfaceId, OwnedNetworkTxFrame, ip::{Configuration, Resources}};
/// use oer_portable_memory::ExecutionContext;
/// use oer_protocols_network_port::{NetDevice, NetTransmit};
/// fn twice<X: ExecutionContext, D: NetDevice, T: NetTransmit<Frame = OwnedNetworkTxFrame> + 'static>(
///     resources: &'static mut Resources<X, 1, T>, first: (D, T), second: (D, T),
///     general: xarxa_driver::PacketBufAllocator,
/// ) {
///     let _ = resources.start::<D, 1>(first.0, first.1, None, general, NetworkInterfaceId::new(0), [2; 6], 1, Configuration::Dhcpv4);
///     let _ = resources.start::<D, 1>(second.0, second.1, None, general, NetworkInterfaceId::new(0), [2; 6], 1, Configuration::Dhcpv4);
/// }
/// ```
pub struct Resources<
    X: ExecutionContext + 'static,
    const RX: usize,
    T: NetTransmit<Frame = OwnedNetworkTxFrame> + 'static,
> {
    rx_storage: PacketPoolStorage<RX>,
    rx_pool: Option<PacketPool<RX>>,
    endpoint: OwnedEndpointResources<X, RX>,
    device: Option<OwnedNetworkDevice<'static, X, RX, T>>,
    stack: StackStorage<'static, OwnedNetworkDevice<'static, X, RX, T>>,
    state: State,
}

/// A pool of `N` packets in its own memory: the general allocator of a
/// stack whose device's transmit half copies each frame it takes, so a sent
/// packet's memory returns to this pool at once.
pub struct GeneralPool<const N: usize> {
    storage: PacketPoolStorage<N>,
    pool: Option<PacketPool<N>>,
}

impl<const N: usize> GeneralPool<N> {
    pub const fn new() -> Self {
        Self {
            storage: PacketPoolStorage::new(),
            pool: None,
        }
    }

    /// The pool's allocator, made once from its placed storage.
    pub fn allocator(&'static mut self) -> PacketBufAllocator {
        let Self { storage, pool } = self;
        let pool: &'static PacketPool<N> = match pool {
            Some(pool) => pool,
            None => pool.insert(PacketPool::new(storage)),
        };
        pool.allocator()
    }
}

impl<const N: usize> Default for GeneralPool<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<
    X: ExecutionContext + 'static,
    const RX: usize,
    T: NetTransmit<Frame = OwnedNetworkTxFrame> + 'static,
> Default for Resources<X, RX, T>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<
    X: ExecutionContext + 'static,
    const RX: usize,
    T: NetTransmit<Frame = OwnedNetworkTxFrame> + 'static,
> Resources<X, RX, T>
{
    pub const fn new() -> Self {
        Self {
            rx_storage: PacketPoolStorage::new(),
            rx_pool: None,
            endpoint: OwnedEndpointResources::new(),
            device: None,
            stack: StackStorage::new(),
            state: State::new(),
        }
    }

    /// Build once over `device`, `transmit`, its transmit half, and
    /// `general`, the allocator of every packet the stack makes, which no
    /// other stack waits on. Errors retain final storage, without a second
    /// stack or fallback configuration.
    #[allow(
        clippy::too_many_arguments,
        reason = "one start over the device and its pools"
    )]
    pub fn start<D: NetDevice, const LENT: usize>(
        &'static mut self,
        device: D,
        transmit: T,
        lent: Option<&'static crate::ethernet::LentSlots<crate::ethernet::Lent<D>, LENT>>,
        general: PacketBufAllocator,
        interface: NetworkInterfaceId,
        address: [u8; 6],
        seed: u64,
        configuration: Configuration,
    ) -> Result<(Control<'static>, Service<X, RX, T, D, LENT>), Error> {
        let Initialized {
            state,
            iface,
            runner,
            radio,
        } = self.initialize(transmit, general, interface, address, seed, configuration)?;
        Ok((
            Control { state },
            Service {
                iface,
                runner,
                bridge: EthernetBridge::new(radio, device, lent),
                configuration,
                state,
            },
        ))
    }

    fn initialize(
        &'static mut self,
        transmit: T,
        general: PacketBufAllocator,
        interface: NetworkInterfaceId,
        address: [u8; 6],
        seed: u64,
        configuration: Configuration,
    ) -> Result<Initialized<X, RX, T>, Error> {
        if let Configuration::Static(config) = configuration
            && config.prefix_length > 32
        {
            return Err(Error::InvalidPrefixLength(config.prefix_length));
        }
        // The stack needs the general allocator's waiter: refuse one another
        // stack holds, before anything is built.
        let pool = ClaimedPool::claim(general).ok_or(Error::GeneralWaiterUnavailable)?;
        let receive = self
            .rx_pool
            .insert(PacketPool::new(&mut self.rx_storage))
            .allocator();
        let (device, radio) = self
            .endpoint
            .split(interface, address, receive, transmit)
            .map_err(Error::from)?;
        let (stack, mut runner) = Stack::new(&mut self.stack, seed, pool);
        runner.set_poll_budget(owned_embassy_net::PollBudget::new(4, 8));
        let iface = stack
            .add_iface_borrowed(self.device.insert(device))
            .map_err(Error::from)?;
        match configuration {
            Configuration::Dhcpv4 => iface
                .set_dhcpv4(Some(DhcpConfig::default()))
                .map_err(|_| Error::DhcpMediumMismatch)?,
            Configuration::Static(config) => {
                let cidr = Ipv4Cidr::new(Ipv4Addr::from(config.address), config.prefix_length);
                iface
                    .set_ip_addrs([IpCidr::V4(cidr)])
                    .map_err(Error::from)?;
                if let Some(gateway) = config.gateway {
                    stack
                        .routes()
                        .add_default_ipv4_route(Ipv4Addr::from(gateway), iface.handle())
                        .map_err(Error::from)?;
                }
            }
        }
        Ok(Initialized {
            state: &self.state,
            iface,
            runner,
            radio,
        })
    }
}

struct Initialized<
    X: ExecutionContext + 'static,
    const RX: usize,
    T: NetTransmit<Frame = OwnedNetworkTxFrame> + 'static,
> {
    state: &'static State,
    iface: Iface<'static, OwnedNetworkDevice<'static, X, RX, T>>,
    runner: owned_embassy_net::Runner<'static, OwnedNetworkDevice<'static, X, RX, T>>,
    radio: crate::OwnedNetworkRunner<'static, X, RX>,
}

/// Permanent bridge and stack runner. Reconnect never reconstructs this value.
/// After stop/error it remains terminal, retaining the stack and device owners.
/// `LENT` counts the packet slots of the device's lent frames.
pub struct Service<
    X: ExecutionContext + 'static,
    const RX: usize,
    T: NetTransmit<Frame = OwnedNetworkTxFrame> + 'static,
    D: NetDevice,
    const LENT: usize,
> {
    iface: Iface<'static, OwnedNetworkDevice<'static, X, RX, T>>,
    runner: owned_embassy_net::Runner<'static, OwnedNetworkDevice<'static, X, RX, T>>,
    bridge: EthernetBridge<'static, X, RX, D, LENT>,
    configuration: Configuration,
    state: &'static State,
}

impl<
    X: ExecutionContext + 'static,
    const RX: usize,
    T: NetTransmit<Frame = OwnedNetworkTxFrame> + 'static,
    D: NetDevice,
    const LENT: usize,
> Service<X, RX, T, D, LENT>
{
    pub fn stack(&self) -> Stack<'static, OwnedNetworkDevice<'static, X, RX, T>> {
        self.iface.stack()
    }

    pub fn interface(&self) -> Iface<'static, OwnedNetworkDevice<'static, X, RX, T>> {
        self.iface
    }

    /// Poll the bridge before the pinned stack, then report every observed
    /// configuration change, including renewal while continuously configured.
    /// Only device/stack events, protocol deadlines and explicit stop wake it.
    pub async fn run(
        &mut self,
        mut events: impl FnMut(StatusChanged),
        mut received: impl FnMut(EthernetFrameView<'_>),
        mut refused: impl FnMut(crate::RxEnqueueError),
        mut copied: impl FnMut(crate::ethernet::LentCopy),
    ) -> Result<(), Error> {
        let mut running = core::pin::pin!(self.runner.run());
        drive(
            self.state,
            &mut self.bridge,
            &self.iface,
            self.configuration,
            |cx| {
                let _ = running.as_mut().poll(cx);
            },
            &mut events,
            &mut received,
            &mut refused,
            &mut copied,
        )
        .await
    }
}

// The pinned stack supplies lease observations; the service owns epoch,
// event ordering and shutdown. Tests supply stack events without installing
// Embassy's process-global time ABI or using wall-clock time.
trait Interface {
    fn restart_dhcp(&self);
    fn lease(&self) -> Option<Ipv4Configuration>;
    fn clear(&self) -> Result<(), Error>;
    /// Drop every received packet the stack has not taken yet, every reply
    /// it retains and every packet its sockets queue: a lent one returns to
    /// its device.
    fn drop_received(&self) {}
}
impl<X: ExecutionContext, const RX: usize, T: NetTransmit<Frame = OwnedNetworkTxFrame>> Interface
    for Iface<'_, OwnedNetworkDevice<'_, X, RX, T>>
{
    fn restart_dhcp(&self) {
        self.restart_dhcpv4();
    }
    fn drop_received(&self) {
        self.with_driver(|driver| while driver.receive().is_some() {});
        // A reply reuses its request's packet: a lent one goes back too.
        self.release_retained();
        // A UDP or raw socket queues the received packets themselves: the
        // stopped stack closes them, whoever still holds the socket.
        self.stack().close_packet_sockets();
    }
    fn lease(&self) -> Option<Ipv4Configuration> {
        self.dhcpv4_lease().map(|lease| Ipv4Configuration {
            address: lease.address.address().octets(),
            prefix_length: lease.address.prefix_len(),
            gateway: lease.router.map(|router| router.octets()),
        })
    }
    fn clear(&self) -> Result<(), Error> {
        let dhcp = self.set_dhcpv4(None).map_err(|_| Error::DhcpMediumMismatch);
        let address = self.set_ip_addrs([]).map_err(Error::from);
        self.stack().routes().clear();
        dhcp.and(address)
    }
}

#[allow(clippy::too_many_arguments)]
async fn drive<'r, X: ExecutionContext, const RX: usize, D: NetDevice, const LENT: usize>(
    state: &State,
    bridge: &mut EthernetBridge<'r, X, RX, D, LENT>,
    iface: &impl Interface,
    configuration: Configuration,
    mut poll_stack: impl FnMut(&mut core::task::Context<'_>),
    mut events: impl FnMut(StatusChanged),
    mut received: impl FnMut(EthernetFrameView<'_>),
    mut refused: impl FnMut(crate::RxEnqueueError),
    mut copied: impl FnMut(crate::ethernet::LentCopy),
) -> Result<(), Error> {
    core::future::poll_fn(|cx| {
        state.runner.register(cx.waker());
        match state.status.get().lifecycle {
            Lifecycle::Stopped => return Poll::Ready(Ok(())),
            Lifecycle::Failed(error) => return Poll::Ready(Err(error)),
            Lifecycle::Running => {}
        }
        if state.stop.get() {
            return finish(state, bridge, iface, &mut events);
        }
        bridge.poll(cx, &mut received, &mut refused, &mut copied);
        let link = bridge.link();
        let previous = state.status.get();
        if link != previous.link {
            // A complete down/up may coalesce into Up with a new epoch.
            // The stack sees only its driver's level, so reset explicitly
            // before its next poll, and publish loss before any new lease.
            if matches!(configuration, Configuration::Dhcpv4) {
                iface.restart_dhcp();
            }
            state.publish(
                Status {
                    link,
                    ipv4: Ipv4Status::Unconfigured,
                    ..previous
                },
                &mut events,
            );
        }
        poll_stack(cx);
        // The stack's poll may have dropped lent frames' packets.
        bridge.reclaim_lent();
        let ipv4 = if link.state == LinkState::Down {
            Ipv4Status::Unconfigured
        } else {
            match configuration {
                Configuration::Dhcpv4 => iface
                    .lease()
                    .map_or(Ipv4Status::Unconfigured, Ipv4Status::Lease),
                Configuration::Static(config) => Ipv4Status::Static(config),
            }
        };
        state.publish(
            Status {
                link,
                ipv4,
                lifecycle: Lifecycle::Running,
            },
            &mut events,
        );
        Poll::Pending
    })
    .await
}

fn finish<'r, X: ExecutionContext, const RX: usize, D: NetDevice, const LENT: usize>(
    state: &State,
    bridge: &mut EthernetBridge<'r, X, RX, D, LENT>,
    iface: &impl Interface,
    events: &mut impl FnMut(StatusChanged),
) -> Poll<Result<(), Error>> {
    bridge.stop();
    iface.drop_received();
    // The packets the interface held may be lent frames: return them now,
    // the service polls its bridge no more.
    bridge.reclaim_lent();
    let result = iface.clear();
    state.publish(
        Status {
            link: LinkSnapshot {
                state: LinkState::Down,
                ..state.status.get().link
            },
            ipv4: Ipv4Status::Unconfigured,
            lifecycle: match result {
                Ok(()) => Lifecycle::Stopped,
                Err(error) => Lifecycle::Failed(error),
            },
        },
        events,
    );
    Poll::Ready(result)
}

#[cfg(test)]
#[allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    reason = "tests build fixed frame fixtures"
)]
mod tests;
