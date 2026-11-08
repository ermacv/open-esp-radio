//! The chip-side names the image's facade (`udp.rs`) uses, over the host's
//! crates, and a network a test builds itself: the service with the load's
//! producer on the sending port and a channel socket on the receiving one.
#![allow(dead_code, reason = "each test uses a part")]

use core::marker::PhantomData;

use oer_portable_memory::WakingSpscSender;
use oer_protocols_network_core::{Group, Interface, NetworkOwner, claim};
use oer_protocols_network_port::{FramePort, FrameSource};
use oer_protocols_network_runtime::{
    Direct, MemoryPolicy, Producer, Service, SocketLanes, StopLane,
};
use oer_protocols_network_wire::ip::Dscp;

pub use oer_portable_memory::{AtomicWaker, ClaimCell};
pub use oer_portable_time::Instant as NetworkInstant;
pub use oer_protocols_network_core::{Configuration, Declined, Ipv4, Outcome, Remote, SendRefusal};
pub use oer_protocols_network_runtime::{
    Interest, LocalEnd, Next, Offer, Produce, Refusal, Refused, WatchRefused, Work,
};

pub struct Brand;
pub const BLOCKS: usize = 64;
pub const BLOCK: usize = 1472;
pub type Socket = oer_protocols_network_runtime::Socket<'static, 'static, Brand, 1472, 32, 32, 32>;
pub type NetworkBlock = oer_protocols_network_core::Block<'static, Brand, 1472>;
pub type Received = oer_protocols_network_core::Datagram<'static, Brand, 1472>;

/// The ports of a network's sockets, as the chip names them.
pub struct Ports {
    pub receiving: u16,
    pub sending: u16,
}

/// A network the test built: its address is the test's to set.
pub struct Network<P> {
    pub end: &'static LocalEnd,
    pub configuration: std::rc::Rc<core::cell::Cell<Option<Configuration>>>,
    pub producer: PhantomData<P>,
}

impl<P> Network<P> {
    pub const fn end(&self) -> &'static LocalEnd {
        self.end
    }

    pub fn configuration(&self) -> Result<Option<Configuration>, WatchRefused> {
        Ok(self.configuration.get())
    }
}

/// An address the test's network holds.
pub fn holding(address: [u8; 4]) -> Option<Configuration> {
    Some(Configuration {
        ipv4: Some(Ipv4::new(address, 24, None).unwrap()),
        version: 1,
        lease: None,
        dhcp: None,
        link: oer_protocols_network_port::LinkSnapshot {
            state: oer_protocols_network_port::LinkState::Up,
            epoch: 1,
        },
    })
}

/// The image's service over `port`, as the chip's network types it.
pub(crate) type Served<P> = Service<
    'static,
    'static,
    Brand,
    1472,
    2,
    32,
    8,
    4,
    64,
    32,
    32,
    P,
    Direct,
    Producer<'static, Brand, crate::udp::LoadProducer, 32>,
>;

/// What [`network`] builds.
pub(crate) struct Built<P: FramePort + FrameSource> {
    pub service: Served<P>,
    /// What the facade reads as the network's address.
    pub address: std::rc::Rc<core::cell::Cell<Option<Configuration>>>,
    pub net: crate::udp::Network,
    pub stop: WakingSpscSender<'static, (), 1>,
    pub end: &'static LocalEnd,
    pub source: &'static crate::udp::Source,
}

/// A service over `port` at `ours`, whose producer sends from `sending` and
/// whose channel socket receives on `receiving`, as the chip's
/// `Network::start` builds it, and the facade over it, which reads `ours` as
/// the network's address until the test sets another.
pub(crate) fn network<P: FramePort + FrameSource>(
    port: P,
    hardware: [u8; 6],
    ours: [u8; 4],
    receiving: u16,
    sending: u16,
) -> Built<P> {
    let memory = Box::leak(Box::new([[0; 1472]; 64]));
    let (pool, brand) = claim::<Brand, 1472, 64>(memory).unwrap();
    let interface = Interface::new(hardware, ours, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(brand, interface, pool, port);
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let end: &'static LocalEnd = Box::leak(Box::new(LocalEnd::new()));
    let source: &'static crate::udp::Source = Box::leak(Box::new(crate::udp::Source::new()));
    let mut service = Service::with_producer(
        owner,
        Direct,
        MemoryPolicy::KEEP,
        stopping,
        crate::udp::LoadProducer::new(source),
        end,
    );
    service.bind_local(sending, Dscp::default()).unwrap();
    let socket = service
        .bind(
            receiving,
            Dscp::default(),
            Group::shared(),
            Box::leak(Box::new(SocketLanes::new())),
        )
        .ok()
        .unwrap();
    let address = std::rc::Rc::new(core::cell::Cell::new(holding(ours)));
    let net = crate::udp::Network::new(
        Network {
            end,
            configuration: address.clone(),
            producer: PhantomData,
        },
        socket,
        source,
    );
    Built {
        service,
        address,
        net,
        stop,
        end,
        source,
    }
}
