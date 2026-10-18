//! Reproduce pre-radio prefix loss using the exact owned stack pinned by HIL.
#![cfg(feature = "owned-network")]
use std::{cell::RefCell, rc::Rc};
use xarxa_owned::{
    Stack,
    driver::{
        Capabilities, Driver, HardwareAddress, LinkState, PacketBuf, PacketPool, PacketPoolStorage,
    },
    time::{Duration, Instant},
    wire::{
        EthernetAddress, EthernetFrame, IpCidr, Ipv4Addr, Ipv4Packet, ListenSocketAddr, SocketAddr,
        UdpPacket,
    },
};

struct Device(Rc<RefCell<Vec<Vec<u8>>>>);
impl Driver for Device {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn hardware_address(&self) -> HardwareAddress {
        HardwareAddress::Ethernet([2, 0, 0, 0, 0, 1])
    }
    fn link_state(&mut self) -> LinkState {
        LinkState::Up
    }
    fn receive(&mut self) -> Option<PacketBuf> {
        None
    }
    fn can_transmit(&mut self) -> bool {
        true
    }
    fn transmit(&mut self, packet: PacketBuf) -> Result<(), PacketBuf> {
        self.0.borrow_mut().push(packet.to_vec());
        Ok(())
    }
}

fn burst(resolve_before: bool) -> Vec<u32> {
    let storage = Box::leak(Box::new(PacketPoolStorage::<64>::new()));
    let pool = Box::leak(Box::new(PacketPool::new(storage)));
    let frames = Rc::new(RefCell::new(Vec::new()));
    let mut device = Device(frames.clone());
    let mut stack = Stack::new(1, pool.allocator());
    let iface = stack.add_iface_borrowed(&mut device).unwrap();
    stack
        .iface(iface)
        .add_ip_addr(IpCidr::new(Ipv4Addr::new(10, 43, 0, 1).into(), 24))
        .unwrap();
    stack.poll(Instant::ZERO);
    let peer = Ipv4Addr::new(10, 43, 0, 2);
    let resolve = |stack: &mut Stack<'_>| {
        // Model receipt of the ARP mapping, without any radio or host UDP queue.
        stack
            .neighbor_cache_mut()
            .insert(
                iface,
                peer.into(),
                xarxa_owned::wire::HardwareAddress::Ethernet(EthernetAddress([2, 0, 0, 0, 0, 2])),
                // Xarxa's clock is wrapping milliseconds: the farthest
                // expiry a comparison still orders after now.
                Instant::ZERO + Duration::MAX,
            )
            .expect("the peer and its hardware address are unicast");
    };
    if resolve_before {
        resolve(&mut stack);
    }
    let socket = stack.add_udp_socket().unwrap();
    stack
        .udp_socket(socket)
        .bind(4324, ListenSocketAddr::UNSPECIFIED)
        .unwrap();
    for sequence in 0_u32..128 {
        stack
            .udp_socket(socket)
            .send_slice(&sequence.to_be_bytes(), SocketAddr::new(peer.into(), 9002))
            .unwrap();
    }
    resolve(&mut stack);
    stack.poll(Instant::from_millis(1));
    frames
        .borrow_mut()
        .iter_mut()
        .filter_map(|bytes| {
            let mut ethernet = EthernetFrame::new_checked(bytes.as_mut_slice()).ok()?;
            if ethernet.ethertype() != xarxa_owned::wire::EthernetProtocol::Ipv4 {
                return None;
            }
            let mut ip = Ipv4Packet::new_checked(ethernet.payload_mut()).ok()?;
            let udp = UdpPacket::new_checked(ip.payload_mut()).ok()?;
            Some(u32::from_be_bytes(udp.payload().try_into().ok()?))
        })
        .collect()
}

#[test]
fn cold_neighbor_loses_exactly_the_first_112_of_a_128_packet_burst() {
    assert_eq!(burst(false), (112..128).collect::<Vec<_>>());
}

#[test]
fn confirmed_neighbor_delivers_sequence_zero_and_the_entire_burst() {
    assert_eq!(burst(true), (0..128).collect::<Vec<_>>());
}
