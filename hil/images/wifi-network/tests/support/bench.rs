//! A port that takes frames once open, and the helpers the facade's tests
//! share.
#![allow(dead_code, reason = "each test uses a part")]

use core::future::Future;
use core::task::{Context, Waker};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use oer_portable_time_virtual::VirtualClock;
use oer_protocols_network_port::{EthernetFrameView, FramePort, FrameSource, Unpublished};
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::{DsField, Ipv4Fields, Ipv4Header};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UdpHeader};

use crate::{Start, StaticAddress, header, udp, wifi};

pub(crate) const OURS: [u8; 4] = [192, 0, 2, 1];
pub(crate) const THEIRS: [u8; 4] = [192, 0, 2, 2];
/// The peer's ARP answer, which the tests' ports give the core before any
/// datagram, so that it sends at once.
pub(crate) const PEER: [u8; 6] = [4; 6];
pub(crate) const LOCAL: [u8; 6] = [2; 6];

/// Frames waiting to be read, and those the port took while open.
#[derive(Default)]
pub(crate) struct Wire {
    pub(crate) incoming: VecDeque<Vec<u8>>,
    pub(crate) sent: Vec<Vec<u8>>,
    /// The port takes frames.
    pub(crate) open: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Port(pub(crate) Rc<RefCell<Wire>>);
impl FramePort for Port {
    type Refusal = ();
    type Ticket = ();
    fn publish<E>(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), E>,
    ) -> Result<(), Unpublished<(), E>> {
        let mut wire = self.0.borrow_mut();
        if !wire.open {
            return Err(Unpublished::Port(()));
        }
        let mut frame = vec![0; length];
        fill(&mut frame).map_err(Unpublished::Fill)?;
        wire.sent.push(frame);
        Ok(())
    }
}
impl FrameSource for Port {
    type Notice = ();
    fn take_frame(&mut self, read: impl FnOnce(EthernetFrameView<'_>)) -> bool {
        let Some(frame) = self.0.borrow_mut().incoming.pop_front() else {
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
pub(crate) type TestService = wifi::Served<Port>;
pub(crate) fn turn(service: &mut TestService, clock: &VirtualClock) {
    let mut events = ();
    let mut run = core::pin::pin!(service.run(clock, &mut events));
    assert!(
        run.as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
}

/// A datagram for this network's first port.
pub(crate) fn datagram() -> Vec<u8> {
    let (ours, theirs) = (OURS, THEIRS);
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
        destination: LOCAL,
        source: PEER,
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

/// The peer's ARP request for us, so the core knows it.
pub(crate) fn peer_asks() -> Vec<u8> {
    use oer_protocols_network_wire::arp::{self, Arp, Operation};
    let mut frame = EthernetHeader {
        destination: [0xFF; 6],
        source: PEER,
        ether_type: EtherType::ARP,
    }
    .to_bytes()
    .to_vec();
    let mut packet = [0; arp::PACKET_LEN];
    Arp {
        operation: Operation::Request,
        sender_hardware: PEER,
        sender_protocol: THEIRS,
        target_hardware: [0; 6],
        target_protocol: OURS,
    }
    .write(&mut packet)
    .unwrap();
    frame.extend_from_slice(&packet);
    frame
}

pub(crate) fn start() -> Start {
    Start {
        address: Some(StaticAddress {
            address: OURS,
            prefix: 24,
            gateway: THEIRS,
        }),
        port: 5001,
    }
}

pub(crate) const PEER_PORT: udp::Peer = udp::Peer {
    address: THEIRS,
    port: 5001,
};

pub(crate) fn load(run: u32, sequence: u64) -> header::Header {
    header::Header {
        run,
        sequence,
        sent_us: 1000 + sequence,
        echo_request: false,
        echo_reply: false,
    }
}

/// The load datagrams the port took, as UDP payloads from the sending port.
pub(crate) fn sent(port: &Port) -> Vec<Vec<u8>> {
    port.0
        .borrow()
        .sent
        .iter()
        .filter_map(|frame| {
            let (ethernet, packet) = EthernetHeader::parse(frame).ok()?;
            (ethernet.ether_type == EtherType::IPV4).then_some(())?;
            let ip = Ipv4Header::parse(packet).ok()?;
            let (udp, data) = UdpHeader::parse(ip.payload()).ok()?;
            assert!(
                Checksum::new()
                    .with(&ipv4_pseudo_header(OURS, THEIRS, PROTOCOL_UDP, udp.length))
                    .with(ip.payload())
                    .verifies()
            );
            (udp.ports.source == 5002).then(|| data.to_vec())
        })
        .collect()
}
