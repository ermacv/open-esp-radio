//! The image's UDP facade on real sockets: `oer` and the load service stand in
//! for their chip-side names, so only start and the port are substituted.
extern crate self as oer;
extern crate self as oer_hil_services_load;

use core::future::Future;
use core::task::{Context, Waker};
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_network_core::{Group, Interface, NetworkOwner, claim};
use oer_protocols_network_port::{EthernetFrameView, FramePort, FrameSource, Unpublished};
use oer_protocols_network_runtime::{Direct, MemoryPolicy, Service, SocketLanes, StopLane};
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::{DsField, Dscp, Ipv4Fields};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UdpHeader};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticAddress {
    pub address: [u8; 4],
    pub prefix: u8,
    pub gateway: [u8; 4],
}
pub struct Start {
    pub address: Option<StaticAddress>,
    pub port: u16,
}
#[derive(Debug)]
pub enum LoadError {
    NoAddress,
    Busy,
    InvalidParameters,
    Bind,
}

pub mod wifi {
    pub struct Brand;
    pub const SOCKETS: usize = 2;
    pub const BLOCKS: usize = 64;
    pub const RESERVE_AHEAD: usize = oer_protocols_network_runtime::RESERVE_AHEAD;
    pub type Socket =
        oer_protocols_network_runtime::Socket<'static, 'static, Brand, 1472, 32, 32, 32>;
    pub type NetworkBlock = oer_protocols_network_core::Block<'static, Brand, 1472>;
    pub type Received = oer_protocols_network_core::Datagram<'static, Brand, 1472>;
    pub use oer_protocols_network_core::Remote;
    pub use oer_protocols_network_runtime::{Refusal, Refused};
    pub struct NetworkAddress {
        pub address: [u8; 4],
        pub prefix: u8,
        pub gateway: Option<[u8; 4]>,
    }
    pub struct Network {
        pub sockets: Option<[Socket; 2]>,
    }
    impl Network {
        pub fn configure(&mut self, _: NetworkAddress, _: [u16; 2]) -> Result<[Socket; 2], ()> {
            self.sockets.take().ok_or(())
        }
    }
}

#[path = "../src/udp.rs"]
mod udp;

/// Frames waiting to be read; it publishes nothing.
#[derive(Clone, Default)]
struct Port(Rc<RefCell<VecDeque<Vec<u8>>>>);
impl FramePort for Port {
    type Refusal = ();
    type Ticket = ();
    fn publish<E>(
        &mut self,
        _: usize,
        _: impl FnOnce(&mut [u8]) -> Result<(), E>,
    ) -> Result<(), Unpublished<(), E>> {
        Err(Unpublished::Port(()))
    }
}
impl FrameSource for Port {
    type Notice = ();
    fn take_frame(&mut self, read: impl FnOnce(EthernetFrameView<'_>)) -> bool {
        let Some(frame) = self.0.borrow_mut().pop_front() else {
            return false;
        };
        let (header, payload) = EthernetHeader::parse(&frame).unwrap();
        read(EthernetFrameView {
            destination: header.destination,
            source: header.source,
            ether_type: header.ether_type.value(),
            payload,
        });
        true
    }
    fn take_notice(&mut self) -> Option<()> {
        None
    }
    fn register(&mut self, _: &mut Context<'_>) {}
}
type TestService =
    Service<'static, 'static, wifi::Brand, 1472, 2, 32, 8, 4, 64, 32, 32, Port, Direct>;
fn turn(service: &mut TestService, clock: &VirtualClock) {
    let mut events = ();
    let mut run = core::pin::pin!(service.run(clock, &mut events));
    assert!(
        run.as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
}

/// A datagram for this network's first port.
fn datagram() -> Vec<u8> {
    let (ours, theirs) = ([192, 0, 2, 1], [192, 0, 2, 2]);
    let data = [7; 16];
    let length = 8 + data.len() as u16;
    let unfilled = UdpHeader {
        ports: Ports {
            source: 5001,
            destination: 5001,
        },
        length,
        checksum: 0,
    };
    let checksum = udp_field(
        Checksum::new()
            .with(&ipv4_pseudo_header(theirs, ours, PROTOCOL_UDP, length))
            .with(&unfilled.to_bytes())
            .with(&data)
            .finish(),
    );
    let mut frame = EthernetHeader {
        destination: [2; 6],
        source: [4; 6],
        ether_type: EtherType::IPV4,
    }
    .to_bytes()
    .to_vec();
    frame.extend_from_slice(
        &Ipv4Fields {
            services: DsField::from_bytes([0]),
            total_length: length + 20,
            identification: 1,
            time_to_live: 64,
            protocol: PROTOCOL_UDP,
            source: theirs,
            destination: ours,
        }
        .to_bytes(),
    );
    frame.extend_from_slice(
        &UdpHeader {
            checksum,
            ..unfilled
        }
        .to_bytes(),
    );
    frame.extend_from_slice(&data);
    frame
}

#[test]
fn ending_a_load_run_returns_every_block_its_adapters_held() {
    let memory = Box::leak(Box::new([[0; 1472]; 64]));
    let (pool, brand) = claim::<wifi::Brand, 1472, 64>(memory).unwrap();
    let interface = Interface::new([2; 6], [192, 0, 2, 1], 24, None, 1500).unwrap();
    let port = Port::default();
    let owner = NetworkOwner::new(brand, interface, pool, port.clone());
    let (_, stop) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::new(owner, Direct, MemoryPolicy::KEEP, stop);
    let lanes1 = Box::leak(Box::new(SocketLanes::new()));
    let lanes2 = Box::leak(Box::new(SocketLanes::new()));
    let one = service
        .bind(5001, Dscp::default(), Group::shared(), lanes1)
        .ok()
        .unwrap();
    let two = service
        .bind(5002, Dscp::default(), Group::shared(), lanes2)
        .ok()
        .unwrap();
    let net = udp::Network::new(wifi::Network {
        sockets: Some([one, two]),
    });
    let start = Start {
        address: Some(StaticAddress {
            address: [192, 0, 2, 1],
            prefix: 24,
            gateway: [192, 0, 2, 2],
        }),
        port: 5001,
    };
    let mut run = udp::open(&net, &start).unwrap();
    let clock = VirtualClock::new();
    // A run's end finds a datagram its reader had taken and not read.
    port.0.borrow_mut().push_back(datagram());
    turn(&mut service, &clock);
    {
        let (mut receiving, _sending) = run.split();
        let ready = core::pin::pin!(receiving.ready());
        let waker = Waker::noop();
        assert!(ready.poll(&mut Context::from_waker(waker)).is_ready());
    }
    let peer = udp::Peer {
        address: [192, 0, 2, 2],
        port: 5001,
    };
    {
        let (_receiving, mut sending) = run.split();
        let mut taken = 0;
        for _ in 0..1000 {
            match sending.try_send(16, peer, |_| Some(())) {
                udp::Attempt::Taken => taken += 1,
                udp::Attempt::NoRoom => turn(&mut service, &clock),
                udp::Attempt::Lost => panic!("unexpected rejection"),
            }
            if taken == 32 {
                break;
            }
        }
        assert_eq!(taken, 32, "all 32 command credits used");
        // The accepted commands wait for ARP; no command credit is free.
        // Complete the block reservations, then encounter that backpressure.
        for _ in 0..8 {
            turn(&mut service, &clock);
            assert!(matches!(
                sending.try_send(16, peer, |_| Some(())),
                udp::Attempt::NoRoom
            ));
        }
    }
    drop(run);
    let mut ended = service.retire();
    drop(net);
    ended.collect();
    assert_eq!(
        ended.retired.pool.outstanding(),
        0,
        "ending a run lost a block"
    );
}
