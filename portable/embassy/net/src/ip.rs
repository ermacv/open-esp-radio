//! One permanent IP stack over an affine [`NetDevice`].
//!
//! Place [`Resources`] once, then call [`Resources::start`]. Reassociation
//! changes the link epoch and invalidates DHCP, without reconstructing pools,
//! the stack, sockets or the Ethernet bridge. Socket applications use the pinned
//! stack returned by [`Service::stack`]; this module defines no socket API.

use core::{cell::Cell, future::Future, task::Poll};
use oer_memory::ExecutionContext;
use oer_network_port::{EthernetFrameView, LinkSnapshot, LinkState, NetDevice, NetworkInterfaceId};
use oer_sync::PriorityWaker;
use owned_embassy_net::{
    Stack, StackStorage,
    iface::{Iface, dhcpv4::DhcpConfig},
    wire::{IpCidr, Ipv4Addr, Ipv4Cidr},
};
use xarxa_driver::{PacketPool, PacketPoolStorage};

use crate::{OwnedEndpointResources, OwnedNetworkDevice, ethernet::EthernetBridge};

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
    InterfaceTableFull,
    UnsupportedMedium,
    HardwareAddressMismatch,
    AddressNotUnicast,
    AddressTableFull,
    GatewayNotUnicast,
    RouteTableFull,
    DhcpMediumMismatch,
    ContextAccessRefused,
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

/// Final storage for one stack and its two private packet pools. Consuming the
/// exclusive static borrow in `start` prevents a second start or pool waiter.
///
/// Memory placement belongs to the image. There are GENERAL general packet
/// slots, RX receive packet/queue slots and TX software admission credits.
///
/// ```compile_fail,E0499
/// use oer_embassy_net::{NetworkInterfaceId, ip::{Configuration, Resources}};
/// use oer_memory::ExecutionContext;
/// use oer_network_port::NetDevice;
/// fn twice<X: ExecutionContext, D: NetDevice>(
///     resources: &'static mut Resources<X, 1, 1, 1>, first: D, second: D,
/// ) {
///     let _ = resources.start(first, NetworkInterfaceId::new(0), [2; 6], 1, Configuration::Dhcpv4);
///     let _ = resources.start(second, NetworkInterfaceId::new(0), [2; 6], 1, Configuration::Dhcpv4);
/// }
/// ```
pub struct Resources<
    X: ExecutionContext + 'static,
    const RX: usize,
    const TX: usize,
    const GENERAL: usize,
> {
    general_storage: PacketPoolStorage<GENERAL>,
    general_pool: Option<PacketPool<GENERAL>>,
    rx_storage: PacketPoolStorage<RX>,
    rx_pool: Option<PacketPool<RX>>,
    endpoint: OwnedEndpointResources<X, RX, TX>,
    device: Option<OwnedNetworkDevice<'static, X, RX, TX>>,
    stack: StackStorage<'static, OwnedNetworkDevice<'static, X, RX, TX>>,
    state: State,
}

impl<X: ExecutionContext + 'static, const RX: usize, const TX: usize, const GENERAL: usize> Default
    for Resources<X, RX, TX, GENERAL>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<X: ExecutionContext + 'static, const RX: usize, const TX: usize, const GENERAL: usize>
    Resources<X, RX, TX, GENERAL>
{
    pub const fn new() -> Self {
        const {
            if GENERAL == 0 {
                panic!("IP service requires general packet storage");
            }
        }
        Self {
            general_storage: PacketPoolStorage::new(),
            general_pool: None,
            rx_storage: PacketPoolStorage::new(),
            rx_pool: None,
            endpoint: OwnedEndpointResources::new(),
            device: None,
            stack: StackStorage::new(),
            state: State::new(),
        }
    }

    /// Build once. Errors retain final storage; the device is returned on
    /// construction refusal, without a second stack or fallback configuration.
    pub fn start<D: NetDevice>(
        &'static mut self,
        device: D,
        interface: NetworkInterfaceId,
        address: [u8; 6],
        seed: u64,
        configuration: Configuration,
    ) -> Result<(Control<'static>, Service<X, RX, TX, D>), StartRefused<D>> {
        match self.initialize(interface, address, seed, configuration) {
            Ok(Initialized {
                state,
                iface,
                runner,
                radio,
            }) => Ok((
                Control { state },
                Service {
                    iface,
                    runner,
                    bridge: EthernetBridge::new(radio, device),
                    configuration,
                    state,
                },
            )),
            Err(reason) => Err(StartRefused { device, reason }),
        }
    }

    fn initialize(
        &'static mut self,
        interface: NetworkInterfaceId,
        address: [u8; 6],
        seed: u64,
        configuration: Configuration,
    ) -> Result<Initialized<X, RX, TX>, Error> {
        if let Configuration::Static(config) = configuration
            && config.prefix_length > 32
        {
            return Err(Error::InvalidPrefixLength(config.prefix_length));
        }
        let general = self
            .general_pool
            .insert(PacketPool::new(&mut self.general_storage))
            .allocator();
        let receive = self
            .rx_pool
            .insert(PacketPool::new(&mut self.rx_storage))
            .allocator();
        let (device, radio) = self
            .endpoint
            .split(interface, address, receive)
            .map_err(Error::from)?;
        // Only this constructor can reach the fresh general pool: no other
        // async stack can claim its waiter before Stack::new.
        let (stack, mut runner) = Stack::new(&mut self.stack, seed, general);
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

pub struct StartRefused<D> {
    pub device: D,
    pub reason: Error,
}

struct Initialized<X: ExecutionContext + 'static, const RX: usize, const TX: usize> {
    state: &'static State,
    iface: Iface<'static, OwnedNetworkDevice<'static, X, RX, TX>>,
    runner: owned_embassy_net::Runner<'static, OwnedNetworkDevice<'static, X, RX, TX>>,
    radio: crate::OwnedNetworkRunner<'static, X, RX, TX>,
}

/// Permanent bridge and stack runner. Reconnect never reconstructs this value.
/// After stop/error it remains terminal, retaining the stack and device owners.
pub struct Service<X: ExecutionContext + 'static, const RX: usize, const TX: usize, D: NetDevice> {
    iface: Iface<'static, OwnedNetworkDevice<'static, X, RX, TX>>,
    runner: owned_embassy_net::Runner<'static, OwnedNetworkDevice<'static, X, RX, TX>>,
    bridge: EthernetBridge<'static, X, RX, TX, D>,
    configuration: Configuration,
    state: &'static State,
}

impl<X: ExecutionContext + 'static, const RX: usize, const TX: usize, D: NetDevice>
    Service<X, RX, TX, D>
{
    pub fn stack(&self) -> Stack<'static, OwnedNetworkDevice<'static, X, RX, TX>> {
        self.iface.stack()
    }

    pub fn interface(&self) -> Iface<'static, OwnedNetworkDevice<'static, X, RX, TX>> {
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
        mut transmitted: impl FnMut(EthernetFrameView<'_>),
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
            &mut transmitted,
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
}
impl<X: ExecutionContext, const RX: usize, const TX: usize> Interface
    for Iface<'_, OwnedNetworkDevice<'_, X, RX, TX>>
{
    fn restart_dhcp(&self) {
        self.restart_dhcpv4();
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
async fn drive<X: ExecutionContext, const RX: usize, const TX: usize, D: NetDevice>(
    state: &State,
    bridge: &mut EthernetBridge<'_, X, RX, TX, D>,
    iface: &impl Interface,
    configuration: Configuration,
    mut poll_stack: impl FnMut(&mut core::task::Context<'_>),
    mut events: impl FnMut(StatusChanged),
    mut received: impl FnMut(EthernetFrameView<'_>),
    mut refused: impl FnMut(crate::RxEnqueueError),
    mut transmitted: impl FnMut(EthernetFrameView<'_>),
) -> Result<(), Error> {
    core::future::poll_fn(|cx| {
        state.runner.register(cx.waker());
        match state.status.get().lifecycle {
            Lifecycle::Stopped => return Poll::Ready(Ok(())),
            Lifecycle::Failed(error) => return Poll::Ready(Err(error)),
            Lifecycle::Running => {}
        }
        if state.stop.get() {
            return finish(state, bridge, iface, Ok(()), &mut events);
        }
        if let Poll::Ready(Err(_)) = bridge.poll(cx, &mut received, &mut refused, &mut transmitted)
        {
            return finish(
                state,
                bridge,
                iface,
                Err(Error::ContextAccessRefused),
                &mut events,
            );
        }
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

fn finish<X: ExecutionContext, const RX: usize, const TX: usize, D: NetDevice>(
    state: &State,
    bridge: &mut EthernetBridge<'_, X, RX, TX, D>,
    iface: &impl Interface,
    result: Result<(), Error>,
    events: &mut impl FnMut(StatusChanged),
) -> Poll<Result<(), Error>> {
    let result = result.and(bridge.stop().map_err(|_| Error::ContextAccessRefused));
    let result = result.and(iface.clear());
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
#[allow(clippy::disallowed_macros, clippy::disallowed_methods)]
mod tests;
