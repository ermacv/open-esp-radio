//! The DHCP client through the owner's public calls: link, input, tick,
//! demands and transmit. Ported from Xarxa fbebde5's `iface::dhcpv4` tests
//! where they fit, with the times this client's jitter and acceptance points
//! give.

use super::*;
use crate::{Configuration, DhcpDrop, DhcpState, Ipv4, LeaseFault};
use oer_portable_wire::Writer;
use oer_protocols_network_port::Class;
use oer_protocols_network_wire::dhcp::{
    self as wire, DhcpMessage, Fixed, Flags, MessageType, Op, option,
};

const SERVER: [u8; 6] = [2, 0, 0, 0, 0, 0x99];
const SERVER_IP: [u8; 4] = [192, 0, 2, 254];
const OFFERED: [u8; 4] = [192, 0, 2, 50];
const DNS: [u8; 4] = [1, 1, 1, 1];
const SEED: u64 = 0x5EED;

fn dhcp_interface() -> Interface {
    Interface::dhcp(LOCAL, 1500, SEED).unwrap()
}

macro_rules! leasing {
    ($test:expr) => {{
        brand!(Net);
        run::<Net>(dhcp_interface(), $test);
    }};
}

fn secs(seconds: u32) -> Instant {
    at(seconds * 1000)
}

/// What the client sent: its frame's addresses and its message.
#[derive(Debug)]
struct Sent {
    hardware: [u8; 6],
    source: [u8; 4],
    destination: [u8; 4],
    message: Vec<u8>,
}

impl Sent {
    fn parse(frame: &[u8]) -> Option<Self> {
        let (ethernet, payload) = EthernetHeader::parse(frame).ok()?;
        assert_eq!(ethernet.source, LOCAL);
        if ethernet.ether_type != EtherType::IPV4 {
            return None;
        }
        let ip = Ipv4Header::parse(payload).unwrap();
        assert!(Checksum::new().with(&payload[..20]).verifies());
        let [s0, s1, s2, s3, d0, d1, d2, d3] = ip.addresses();
        let (source, destination) = ([s0, s1, s2, s3], [d0, d1, d2, d3]);
        assert_eq!(ip.protocol(), PROTOCOL_UDP);
        let (udp, data) = UdpHeader::parse(ip.payload()).unwrap();
        assert_eq!(udp.ports.source, wire::CLIENT_PORT);
        assert_eq!(udp.ports.destination, wire::SERVER_PORT);
        assert!(
            Checksum::new()
                .with(&ipv4_pseudo_header(
                    source,
                    destination,
                    PROTOCOL_UDP,
                    udp.length
                ))
                .with(ip.payload())
                .verifies()
        );
        Some(Self {
            hardware: ethernet.destination,
            source,
            destination,
            message: data.to_vec(),
        })
    }

    fn message(&self) -> DhcpMessage<'_> {
        DhcpMessage::parse(&self.message).unwrap()
    }

    fn kind(&self) -> MessageType {
        self.message().message_type().unwrap()
    }

    fn xid(&self) -> u32 {
        self.message().fixed.xid
    }

    fn option(&self, code: u8) -> Option<Vec<u8>> {
        self.message().option(code).map(<[u8]>::to_vec)
    }
}

/// The link's state at the port, then one pass of the owner: link, tick,
/// and every ready work sent. What went out.
fn turn<B>(stack: &mut Stack<B>, now: Instant, events: &mut Dropped) -> Vec<Vec<u8>> {
    stack.observe_link(now);
    stack.tick(now, events);
    let before = stack.port().sent.len();
    for _ in 0..16 {
        let Some(demand) = stack.poll_demand(now) else {
            break;
        };
        stack.transmit(now, demand.grant(), unproduced).unwrap();
    }
    stack.port().sent[before..].to_vec()
}

/// The DHCP messages one pass sent.
fn dhcp_turn<B>(stack: &mut Stack<B>, now: Instant) -> Vec<Sent> {
    turn(stack, now, &mut Dropped::default())
        .iter()
        .filter_map(|frame| Sent::parse(frame))
        .collect()
}

/// The one DHCP message one pass sent.
fn one<B>(stack: &mut Stack<B>, now: Instant) -> Sent {
    let mut sent = dhcp_turn(stack, now);
    assert_eq!(sent.len(), 1, "{sent:?}");
    sent.remove(0)
}

fn quiet<B>(stack: &mut Stack<B>, now: Instant) {
    let sent = dhcp_turn(stack, now);
    assert!(sent.is_empty(), "at {now:?}: {sent:?}");
}

fn link_up<B>(stack: &mut Stack<B>, epoch: u64) {
    stack.port_mut().up = Some(epoch);
}

fn link_down<B>(stack: &mut Stack<B>) {
    stack.port_mut().up = None;
}

fn ack_options() -> Vec<(u8, Vec<u8>)> {
    vec![
        (option::SUBNET_MASK, vec![255, 255, 255, 0]),
        (option::ROUTER, SERVER_IP.to_vec()),
        // A 0.0.0.0 server is dropped, as some servers send one.
        (option::DOMAIN_NAME_SERVER, vec![1, 1, 1, 1, 0, 0, 0, 0]),
        (option::LEASE_TIME, 1000_u32.to_be_bytes().to_vec()),
    ]
}

fn with(options: Vec<(u8, Vec<u8>)>, code: u8, data: Option<Vec<u8>>) -> Vec<(u8, Vec<u8>)> {
    let mut options: Vec<_> = options.into_iter().filter(|(c, _)| *c != code).collect();
    if let Some(data) = data {
        options.push((code, data));
    }
    options
}

/// A server's reply in an Ethernet frame to our station, from the server's
/// address and to `to`.
struct Reply {
    kind: MessageType,
    xid: u32,
    yiaddr: [u8; 4],
    to: [u8; 4],
    from: [u8; 4],
    op: Op,
    chaddr: [u8; 6],
    server: Option<[u8; 4]>,
    options: Vec<(u8, Vec<u8>)>,
}

impl Reply {
    fn new(kind: MessageType, xid: u32) -> Self {
        Self {
            kind,
            xid,
            yiaddr: if kind == MessageType::Nak {
                [0; 4]
            } else {
                OFFERED
            },
            to: if kind == MessageType::Nak {
                [255; 4]
            } else {
                OFFERED
            },
            from: SERVER_IP,
            op: Op::Reply,
            chaddr: LOCAL,
            server: Some(SERVER_IP),
            options: if kind == MessageType::Nak {
                vec![]
            } else {
                ack_options()
            },
        }
    }

    fn frame(&self) -> Vec<u8> {
        let mut message = vec![0; 600];
        let length = {
            let mut writer = Writer::new(&mut message);
            let [c0, c1, c2, c3, c4, c5] = self.chaddr;
            Fixed {
                op: self.op,
                htype: 1,
                hlen: 6,
                hops: 0,
                xid: self.xid,
                secs: 0,
                flags: Flags::from_bytes([0; 2]),
                ciaddr: [0; 4],
                yiaddr: self.yiaddr,
                siaddr: SERVER_IP,
                giaddr: [0; 4],
                chaddr: [c0, c1, c2, c3, c4, c5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            }
            .write(&mut writer)
            .unwrap();
            wire::write_option(&mut writer, option::MESSAGE_TYPE, &[self.kind.encoding()]).unwrap();
            if let Some(server) = self.server {
                wire::write_option(&mut writer, option::SERVER_IDENTIFIER, &server).unwrap();
            }
            for (code, data) in &self.options {
                wire::write_option(&mut writer, *code, data).unwrap();
            }
            wire::write_end(&mut writer).unwrap();
            writer.written()
        };
        message.truncate(length);
        udp_from(SERVER, self.from, self.to, (67, 68), &message)
    }
}

/// A UDP datagram in an Ethernet frame to our station.
fn udp_from(
    hardware: [u8; 6],
    from: [u8; 4],
    to: [u8; 4],
    (source, destination): (u16, u16),
    data: &[u8],
) -> Vec<u8> {
    let length = u16::try_from(8 + data.len()).unwrap();
    let unfilled = UdpHeader {
        ports: Ports {
            source,
            destination,
        },
        length,
        checksum: 0,
    };
    let checksum = udp_field(
        Checksum::new()
            .with(&ipv4_pseudo_header(from, to, PROTOCOL_UDP, length))
            .with(&unfilled.to_bytes())
            .with(data)
            .finish(),
    );
    let mut frame = EthernetHeader {
        destination: LOCAL,
        source: hardware,
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
            source: from,
            destination: to,
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
    frame.extend_from_slice(data);
    frame
}

fn give<B>(stack: &mut Stack<B>, now: Instant, frame: &[u8]) -> Dropped {
    let mut dropped = Dropped::default();
    stack.input(now, frame, &mut dropped);
    dropped
}

fn assert_discover(sent: &Sent) -> u32 {
    assert_eq!(sent.kind(), MessageType::Discover);
    assert_eq!(sent.hardware, [0xFF; 6]);
    assert_eq!(sent.source, [0; 4]);
    assert_eq!(sent.destination, [255; 4]);
    let message = sent.message();
    assert_eq!(message.fixed.op, Op::Request);
    assert_eq!(message.fixed.ciaddr, [0; 4]);
    assert!(!message.fixed.flags.broadcast());
    assert_eq!(&message.fixed.chaddr[..6], &LOCAL);
    assert_eq!(sent.option(option::SERVER_IDENTIFIER), None);
    assert_eq!(sent.option(option::REQUESTED_ADDRESS), None);
    assert_eq!(
        sent.option(option::CLIENT_IDENTIFIER),
        Some(vec![1, 2, 0, 0, 0, 0, 1])
    );
    assert_eq!(
        sent.option(option::PARAMETER_REQUEST_LIST),
        Some(vec![1, 3, 6])
    );
    // RFC 2132, 9.10: the MTU less the largest IPv4 and UDP headers.
    assert_eq!(
        sent.option(option::MAXIMUM_MESSAGE_SIZE),
        Some(1432_u16.to_be_bytes().to_vec())
    );
    sent.xid()
}

/// The REQUEST that answers an OFFER, RFC 2131, Table 5.
fn assert_selecting_request(sent: &Sent, xid: u32) {
    assert_eq!(sent.kind(), MessageType::Request);
    assert_eq!(sent.xid(), xid);
    assert_eq!((sent.source, sent.destination), ([0; 4], [255; 4]));
    assert_eq!(sent.hardware, [0xFF; 6]);
    assert_eq!(sent.message().fixed.ciaddr, [0; 4]);
    assert_eq!(
        sent.option(option::REQUESTED_ADDRESS),
        Some(OFFERED.to_vec())
    );
    assert_eq!(
        sent.option(option::SERVER_IDENTIFIER),
        Some(SERVER_IP.to_vec())
    );
    assert_eq!(
        sent.option(option::PARAMETER_REQUEST_LIST),
        Some(vec![1, 3, 6])
    );
}

/// A renewal (to the server) or a rebinding (broadcast) REQUEST.
fn assert_extension(sent: &Sent, broadcast: bool) -> u32 {
    assert_eq!(sent.kind(), MessageType::Request);
    assert_eq!(sent.source, OFFERED);
    assert_eq!(sent.message().fixed.ciaddr, OFFERED);
    if broadcast {
        assert_eq!((sent.destination, sent.hardware), ([255; 4], [0xFF; 6]));
    } else {
        assert_eq!((sent.destination, sent.hardware), (SERVER_IP, SERVER));
    }
    assert_eq!(sent.option(option::REQUESTED_ADDRESS), None);
    assert_eq!(sent.option(option::SERVER_IDENTIFIER), None);
    sent.xid()
}

fn configuration<B>(stack: &Stack<B>) -> Configuration {
    stack.configuration()
}

fn leased_ipv4() -> Ipv4 {
    Ipv4::new(OFFERED, 24, Some(SERVER_IP)).unwrap()
}

/// DISCOVER, OFFER, REQUEST: all at `now`. The exchange's xid.
fn requesting<B>(stack: &mut Stack<B>, now: Instant) -> u32 {
    link_up(stack, 1);
    let xid = assert_discover(&one(stack, now));
    assert!(
        give(stack, now, &Reply::new(MessageType::Offer, xid).frame())
            .0
            .is_empty()
    );
    assert_selecting_request(&one(stack, now), xid);
    xid
}

/// The ARP probe in `frame`, RFC 5227, 2.1.1: the address it asks for.
fn probe_of(frame: &[u8]) -> Option<[u8; 4]> {
    let (ethernet, payload) = EthernetHeader::parse(frame).ok()?;
    if ethernet.ether_type != EtherType::ARP {
        return None;
    }
    let packet = Arp::parse(payload).unwrap().unwrap();
    assert_eq!(ethernet.destination, [0xFF; 6]);
    assert_eq!(packet.operation, Operation::Request);
    assert_eq!(packet.sender_hardware, LOCAL);
    assert_eq!(packet.sender_protocol, [0; 4]);
    assert_eq!(packet.target_hardware, [0; 6]);
    Some(packet.target_protocol)
}

/// The probes one pass sent.
fn probes_at<B>(stack: &mut Stack<B>, now: Instant) -> Vec<[u8; 4]> {
    turn(stack, now, &mut Dropped::default())
        .iter()
        .filter_map(|frame| probe_of(frame))
        .collect()
}

/// Probe the ACK's address to its end, no host answering: when each probe
/// went, and when the address was bound.
fn probed<B>(stack: &mut Stack<B>) -> (Vec<Instant>, Instant) {
    let mut sent = Vec::new();
    assert_eq!(configuration(stack).dhcp, Some(DhcpState::Probing));
    for _ in 0..8 {
        assert_eq!(configuration(stack).ipv4, None);
        let due = stack.poll_deadline().unwrap();
        for probed in probes_at(stack, due) {
            assert_eq!(probed, OFFERED);
            sent.push(due);
        }
        // Bound, and with T1 or T2 already past, renewing or rebinding.
        if configuration(stack).dhcp != Some(DhcpState::Probing) {
            assert!(configuration(stack).lease.is_some());
            return (sent, due);
        }
    }
    panic!("no binding after the probes: {sent:?}");
}

/// The rest of the probing after a probe at `after`: when each later probe
/// went, and when the address was bound.
fn probed_from<B>(stack: &mut Stack<B>, after: Instant) -> (Vec<Instant>, Instant) {
    let (probes, bound) = probed(stack);
    assert!(probes.iter().all(|probe| *probe > after));
    (probes, bound)
}

/// Bound with `options` on an ACK at `now`, its address probed; when it
/// was bound.
fn bound_at<B>(stack: &mut Stack<B>, now: Instant, options: Vec<(u8, Vec<u8>)>) -> (u32, Instant) {
    let xid = requesting(stack, now);
    let mut ack = Reply::new(MessageType::Ack, xid);
    ack.options = options;
    assert!(give(stack, now, &ack.frame()).0.is_empty());
    let (_, bound) = probed(stack);
    (xid, bound)
}

/// Bound with `options` on an ACK at `now`, its address probed.
fn bound_with<B>(stack: &mut Stack<B>, now: Instant, options: Vec<(u8, Vec<u8>)>) -> u32 {
    bound_at(stack, now, options).0
}

fn bound<B>(stack: &mut Stack<B>, now: Instant) -> u32 {
    bound_with(stack, now, ack_options())
}

/// The server answers our request for its address, so a unicast can go.
fn server_answers_arp<B>(stack: &mut Stack<B>, now: Instant) {
    let reply = arp_frame(Operation::Reply, (SERVER, SERVER_IP), OFFERED);
    stack.input(now, &reply, &mut ());
}

#[test]
fn a_link_up_discovers_at_once_and_an_ack_binds_the_lease() {
    leasing!(|stack| {
        // No link, no message; no address, no datagram.
        quiet(stack, at(0));
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Selecting));
        assert_eq!(configuration(stack).ipv4, None);
        let socket = stack.open(7, best_effort()).unwrap();
        let block = filled(stack.pool(), b"early");
        let refused = stack.send(at(0), &socket, peer(), block).unwrap_err();
        assert_eq!(refused.reason, SendRefusal::NoAddress);
        stack.pool().give(refused.block);

        let xid = requesting(stack, at(1));
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Requesting));
        let version = stack.address_version();
        let changes = stack.configuration_changes();
        // The OFFER came unicast to the offered address, the ACK too.
        assert!(
            give(stack, at(2), &Reply::new(MessageType::Ack, xid).frame())
                .0
                .is_empty()
        );
        // RFC 2131, 4.4.1: the new address is probed before it is used.
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Probing));
        assert_eq!(configuration(stack).ipv4, None);
        assert_eq!(stack.address_version(), version);
        let (probes, bound) = probed(stack);
        let configured = configuration(stack);
        assert_eq!(configured.ipv4, Some(leased_ipv4()));
        assert_eq!(configured.dhcp, Some(DhcpState::Bound));
        assert_ne!(configured.version, version);
        assert_ne!(stack.configuration_changes(), changes);
        let lease = configured.lease.unwrap();
        assert_eq!(lease.dns.as_slice(), &[DNS]);
        assert_eq!(lease.server, SERVER_IP);
        // The lease counts from the REQUEST's acceptance, RFC 2131, 4.4.1.
        assert_eq!(lease.obtained, at(1));
        assert_eq!(lease.renew, at(1).saturating_add(Duration::from_secs(500)));
        assert_eq!(lease.rebind, at(1).saturating_add(Duration::from_secs(875)));
        assert_eq!(
            lease.expires,
            at(1).saturating_add(Duration::from_secs(1000))
        );
        assert_eq!(stack.poll_deadline(), Some(lease.renew));
        // RFC 5227, 1.1: the first probe up to a second after the ACK, the
        // three one to two seconds apart, the binding two seconds after the
        // last.
        assert_eq!(probes.len(), 3, "{probes:?}");
        let gap = |from: Instant, to: Instant| to.as_micros() - from.as_micros();
        assert!(gap(at(2), probes[0]) <= 1_000_000, "{probes:?}");
        for pair in probes.windows(2) {
            assert!((1_000_000..=2_000_000).contains(&gap(pair[0], pair[1])));
        }
        assert_eq!(gap(probes[2], bound), 2_000_000);
        // A send goes from the leased address now.
        let block = filled(stack.pool(), b"late");
        stack.send(bound, &socket, peer(), block).unwrap();
    });
}

#[test]
fn discover_repeats_keep_their_transaction_and_double_to_a_ceiling() {
    leasing!(|stack| {
        link_up(stack, 1);
        let xid = assert_discover(&one(stack, at(0)));
        let mut last = at(0);
        // RFC 2131, 4.1's randomized backoff: each wait is its nominal one
        // less up to a quarter, from 250 ms doubling to 10 s.
        for nominal in [250, 500, 1000, 2000, 4000, 8000, 10_000, 10_000] {
            let due = stack.poll_deadline().unwrap();
            let wait = due.as_micros() - last.as_micros();
            assert!(
                (nominal * 750..=nominal * 1000).contains(&wait),
                "{wait} µs for {nominal} ms"
            );
            quiet(stack, Instant::from_micros(due.as_micros() - 1));
            let sent = one(stack, due);
            assert_eq!(assert_discover(&sent), xid);
            last = due;
        }
    });
}

#[test]
fn a_late_offer_answers_a_repeated_discover() {
    leasing!(|stack| {
        link_up(stack, 1);
        let xid = assert_discover(&one(stack, at(0)));
        for _ in 0..3 {
            let due = stack.poll_deadline().unwrap();
            one(stack, due);
        }
        assert!(
            give(stack, secs(3), &Reply::new(MessageType::Offer, xid).frame())
                .0
                .is_empty()
        );
        assert_selecting_request(&one(stack, secs(3)), xid);
    });
}

#[test]
fn an_unanswered_request_repeats_then_discovery_starts_over() {
    leasing!(|stack| {
        let xid = requesting(stack, at(0));
        let mut last = at(0);
        // 5 s doubling every two tries: 5, 5, 10, 10 s, then 20 s to give up.
        for nominal in [5, 5, 10, 10] {
            let due = stack.poll_deadline().unwrap();
            let wait = due.as_micros() - last.as_micros();
            assert!((nominal * 750_000..=nominal * 1_000_000).contains(&wait));
            assert_selecting_request(&one(stack, due), xid);
            last = due;
        }
        let due = stack.poll_deadline().unwrap();
        let wait = due.as_micros() - last.as_micros();
        assert!((15_000_000..=20_000_000).contains(&wait));
        let sent = one(stack, due);
        let new = assert_discover(&sent);
        assert_ne!(new, xid);
        assert_eq!(configuration(stack).ipv4, None);
        // The new discovery works as the first.
        assert!(
            give(stack, due, &Reply::new(MessageType::Offer, new).frame())
                .0
                .is_empty()
        );
        assert_selecting_request(&one(stack, due), new);
    });
}

#[test]
fn an_ack_after_requests_repeated_still_binds() {
    leasing!(|stack| {
        let xid = requesting(stack, at(0));
        let due = stack.poll_deadline().unwrap();
        one(stack, due);
        give(stack, due, &Reply::new(MessageType::Ack, xid).frame());
        probed(stack);
        let lease = configuration(stack).lease.unwrap();
        // From the first REQUEST of the transaction.
        assert_eq!(lease.obtained, at(0));
    });
}

#[test]
fn a_nak_to_the_request_starts_discovery_over_at_once() {
    leasing!(|stack| {
        let xid = requesting(stack, at(0));
        assert!(
            give(stack, at(1), &Reply::new(MessageType::Nak, xid).frame())
                .0
                .is_empty()
        );
        let new = assert_discover(&one(stack, at(1)));
        assert_ne!(new, xid);
        assert_eq!(configuration(stack).ipv4, None);
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Selecting));
    });
}

#[test]
fn renewal_at_t1_goes_to_the_server_and_an_ack_extends_from_its_request() {
    leasing!(|stack| {
        bound(stack, at(0));
        let version = stack.address_version();
        quiet(stack, at(499_999));
        // T1: the server's neighbor is asked for first, then the REQUEST
        // goes to it.
        let frames = turn(stack, secs(500), &mut Dropped::default());
        assert_eq!(frames.len(), 1, "an ARP request");
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Renewing));
        server_answers_arp(stack, secs(500));
        let sent = one(stack, secs(500));
        let xid = assert_extension(&sent, false);
        // The server extends the lease: no new version, times from now.
        assert!(
            give(stack, secs(501), &Reply::new(MessageType::Ack, xid).frame())
                .0
                .is_empty()
        );
        assert_eq!(stack.address_version(), version);
        let lease = configuration(stack).lease.unwrap();
        assert_eq!(lease.obtained, secs(500));
        assert_eq!(lease.renew, secs(1000));
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Bound));
    });
}

#[test]
fn renewals_then_rebinds_follow_rfc_2131_4_4_5_until_the_lease_expires() {
    leasing!(|stack| {
        bound(stack, at(0));
        // T1 = 500 s, T2 = 875 s, the end at 1000 s.
        turn(stack, secs(500), &mut Dropped::default());
        server_answers_arp(stack, secs(500));
        let first = assert_extension(&one(stack, secs(500)), false);
        // Half the time to T2: 500 + 187.5. The server's entry has expired
        // by then, and is asked for again.
        quiet(stack, at(687_499));
        server_answers_arp(stack, at(687_000));
        let second = assert_extension(&one(stack, at(687_500)), false);
        assert_ne!(first, second, "a new transaction for each try");
        // Half again: 687.5 + 93.75.
        quiet(stack, at(781_249));
        server_answers_arp(stack, at(781_000));
        assert_extension(&one(stack, at(781_250)), false);
        // Half of 93.75 s is under 60 s: 60 s later, which T2 cuts.
        quiet(stack, at(841_249));
        server_answers_arp(stack, at(841_000));
        assert_extension(&one(stack, at(841_250)), false);
        quiet(stack, at(874_999));
        // T2: broadcast, to any server.
        assert_extension(&one(stack, secs(875)), true);
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Rebinding));
        // Half the time left: 875 + 62.5; then 60 s, past the end.
        quiet(stack, at(937_499));
        assert_extension(&one(stack, at(937_500)), true);
        quiet(stack, at(997_499));
        assert_extension(&one(stack, at(997_500)), true);
        // The lease ends at 1000 s, not a moment before.
        quiet(stack, at(999_999));
        assert_eq!(configuration(stack).ipv4, Some(leased_ipv4()));
        let version = stack.address_version();
        let sent = one(stack, secs(1000));
        assert_discover(&sent);
        assert_eq!(configuration(stack).ipv4, None);
        assert_eq!(configuration(stack).lease, None);
        assert_ne!(stack.address_version(), version);
    });
}

#[test]
fn a_rebind_ack_just_before_the_end_renews_the_lease_from_its_request() {
    leasing!(|stack| {
        bound(stack, at(0));
        let xid = assert_extension(&one(stack, secs(875)), true);
        assert!(
            give(stack, secs(900), &Reply::new(MessageType::Ack, xid).frame())
                .0
                .is_empty()
        );
        let lease = configuration(stack).lease.unwrap();
        assert_eq!(lease.obtained, secs(875));
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Bound));
    });
}

#[test]
fn a_nak_to_a_renewal_takes_the_address_away() {
    leasing!(|stack| {
        bound(stack, at(0));
        let xid = assert_extension(&one(stack, secs(875)), true);
        give(stack, secs(876), &Reply::new(MessageType::Nak, xid).frame());
        assert_eq!(configuration(stack).ipv4, None);
        assert_discover(&one(stack, secs(876)));
    });
}

#[test]
fn work_accepted_for_an_address_ends_readdressed_when_it_goes() {
    leasing!(|stack| {
        bound(stack, at(0));
        let socket = stack.open(7, best_effort()).unwrap();
        // The peer is never resolved: the datagram waits.
        let block = filled(stack.pool(), b"waiting");
        let free = stack.pool().free();
        let command = stack.send(secs(999), &socket, peer(), block).unwrap();
        turn(stack, at(999_500), &mut Dropped::default());
        one(stack, secs(1000));
        assert_eq!(finished(stack.take_result(command)), Outcome::Readdressed);
        assert_eq!(stack.pool().free(), free + 1);
        // The socket stays open; a send waits for the next address.
        let block = filled(stack.pool(), b"again");
        let refused = stack.send(secs(1000), &socket, peer(), block).unwrap_err();
        assert_eq!(refused.reason, SendRefusal::NoAddress);
        stack.pool().give(refused.block);
        assert!(stack.receive(&socket).unwrap().is_none());
    });
}

#[test]
fn a_new_address_at_renewal_ends_old_work_and_new_work_goes_from_it() {
    leasing!(|stack| {
        bound(stack, at(0));
        let socket = stack.open(7, best_effort()).unwrap();
        let block = filled(stack.pool(), b"old");
        let command = stack.send(secs(870), &socket, peer(), block).unwrap();
        let xid = assert_extension(&one(stack, secs(875)), true);
        let version = stack.address_version();
        let moved = [192, 0, 2, 77];
        let mut ack = Reply::new(MessageType::Ack, xid);
        ack.yiaddr = moved;
        ack.to = moved;
        give(stack, secs(876), &ack.frame());
        assert_ne!(stack.address_version(), version);
        assert_eq!(finished(stack.take_result(command)), Outcome::Readdressed);
        assert_eq!(configuration(stack).ipv4.unwrap().address(), moved);
        // The socket sends from the new address.
        let block = filled(stack.pool(), b"new");
        stack.send(secs(876), &socket, peer(), block).unwrap();
        let request = arp_frame(Operation::Request, (PEER, PEER_IP), moved);
        stack.input(secs(876), &request, &mut ());
        let frames = turn(stack, secs(876), &mut Dropped::default());
        let datagram = frames
            .iter()
            .find(|frame| frame.len() > 42)
            .expect("the datagram went");
        assert_eq!(&datagram[26..30], &moved);
    });
}

#[test]
fn a_link_lost_and_back_mid_lease_runs_init_reboot_and_keeps_the_address() {
    leasing!(|stack| {
        bound(stack, at(0));
        link_down(stack);
        quiet(stack, secs(10));
        assert_eq!(configuration(stack).ipv4, Some(leased_ipv4()));
        link_up(stack, 2);
        let sent = one(stack, secs(11));
        // RFC 2131, 3.2 (1) and Table 5: broadcast, 'ciaddr' zero, the
        // address requested, no server identifier.
        assert_eq!(sent.kind(), MessageType::Request);
        assert_eq!((sent.source, sent.destination), ([0; 4], [255; 4]));
        assert_eq!(sent.message().fixed.ciaddr, [0; 4]);
        assert_eq!(
            sent.option(option::REQUESTED_ADDRESS),
            Some(OFFERED.to_vec())
        );
        assert_eq!(sent.option(option::SERVER_IDENTIFIER), None);
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Rebooting));
        // The address stays in use meanwhile, RFC 2131, 3.7.
        assert_eq!(configuration(stack).ipv4, Some(leased_ipv4()));
        let version = stack.address_version();
        give(
            stack,
            secs(11),
            &Reply::new(MessageType::Ack, sent.xid()).frame(),
        );
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Bound));
        assert_eq!(stack.address_version(), version);
        assert_eq!(configuration(stack).lease.unwrap().obtained, secs(11));
    });
}

#[test]
fn the_configuration_names_the_link_the_core_last_saw_and_a_change_of_it_moves_the_count() {
    use oer_protocols_network_port::{LinkSnapshot, LinkState};
    leasing!(|stack| {
        assert_eq!(configuration(stack).link, LinkSnapshot::DOWN);
        bound(stack, at(0));
        let up = configuration(stack).link;
        assert_eq!((up.state, up.epoch), (LinkState::Up, 1));
        let changes = stack.configuration_changes();
        link_down(stack);
        quiet(stack, secs(10));
        // The lease and its address stay; the link that carried them went.
        let down = configuration(stack);
        assert_eq!(down.link.state, LinkState::Down);
        assert_eq!(down.ipv4, Some(leased_ipv4()));
        assert_eq!(down.dhcp, Some(DhcpState::Bound));
        assert_ne!(stack.configuration_changes(), changes);
        link_up(stack, 2);
        one(stack, secs(11));
        let again = configuration(stack).link;
        assert_eq!((again.state, again.epoch), (LinkState::Up, 2));
    });
}

/// The ACK of a REQUESTING exchange at `now`: the client probes from here.
fn acked<B>(stack: &mut Stack<B>, now: Instant) -> u32 {
    let xid = requesting(stack, now);
    assert!(
        give(stack, now, &Reply::new(MessageType::Ack, xid).frame())
            .0
            .is_empty()
    );
    assert_eq!(configuration(stack).dhcp, Some(DhcpState::Probing));
    xid
}

/// The first probe of the ACK's address: when it went.
fn first_probe<B>(stack: &mut Stack<B>) -> Instant {
    let due = stack.poll_deadline().unwrap();
    assert_eq!(probes_at(stack, due), [OFFERED]);
    due
}

/// The DECLINE a conflict owes, RFC 2131, Table 5: broadcast from 0, the
/// address and the server named, no parameters asked.
fn assert_decline(sent: &Sent, xid: u32) {
    assert_eq!(sent.kind(), MessageType::Decline);
    assert_eq!(sent.xid(), xid);
    assert_eq!((sent.source, sent.destination), ([0; 4], [255; 4]));
    assert_eq!(sent.hardware, [0xFF; 6]);
    let message = sent.message();
    assert_eq!(message.fixed.ciaddr, [0; 4]);
    assert_eq!(message.fixed.secs, 0);
    assert_eq!(
        sent.option(option::REQUESTED_ADDRESS),
        Some(OFFERED.to_vec())
    );
    assert_eq!(
        sent.option(option::SERVER_IDENTIFIER),
        Some(SERVER_IP.to_vec())
    );
    assert_eq!(sent.option(option::PARAMETER_REQUEST_LIST), None);
    assert_eq!(sent.option(option::MAXIMUM_MESSAGE_SIZE), None);
}

#[test]
fn an_offered_address_another_host_answers_for_is_declined_and_discovery_waits_ten_seconds() {
    leasing!(|stack| {
        let xid = acked(stack, at(0));
        let probed = first_probe(stack);
        let version = stack.address_version();
        let changes = stack.configuration_changes();
        // Its holder answers the probe, RFC 5227, 2.1.1.
        let answer = arp_frame(Operation::Reply, (PEER, OFFERED), [0; 4]);
        let mut dropped = Dropped::default();
        stack.input(probed, &answer, &mut dropped);
        assert_eq!(dropped.0, [NetEvent::AddressInUse]);
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Declining));
        assert_ne!(stack.configuration_changes(), changes);
        assert_decline(&one(stack, probed), xid);
        // The address was never used.
        assert_eq!(configuration(stack).ipv4, None);
        assert_eq!(configuration(stack).lease, None);
        assert_eq!(stack.address_version(), version);
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Selecting));
        // RFC 2131, 3.1 (5): no DISCOVER for ten seconds, then a new
        // transaction.
        let restart = probed.saturating_add(Duration::from_secs(10));
        assert_eq!(stack.poll_deadline(), Some(restart));
        quiet(stack, Instant::from_micros(restart.as_micros() - 1));
        assert_ne!(assert_discover(&one(stack, restart)), xid);
    });
}

#[test]
fn another_host_probing_for_the_offered_address_is_a_conflict() {
    leasing!(|stack| {
        let xid = acked(stack, at(0));
        let probed = first_probe(stack);
        // RFC 5227, 2.1.1: a probe from another station, sender 0, for the
        // same address.
        let probe = arp_frame(Operation::Request, (PEER, [0; 4]), OFFERED);
        let mut dropped = Dropped::default();
        stack.input(probed, &probe, &mut dropped);
        assert_eq!(dropped.0, [NetEvent::AddressInUse]);
        assert_decline(&one(stack, probed), xid);
    });
}

#[test]
fn arp_that_names_no_holder_of_the_offered_address_leaves_the_probing_on() {
    leasing!(|stack| {
        acked(stack, at(0));
        let probed = first_probe(stack);
        for frame in [
            // Our own probe, as a switch may echo it.
            arp_frame(Operation::Request, (LOCAL, [0; 4]), OFFERED),
            // Another host asking for another address, and asking for the
            // offered one from an address of its own.
            arp_frame(Operation::Request, (PEER, PEER_IP), [192, 0, 2, 9]),
            arp_frame(Operation::Request, (PEER, PEER_IP), OFFERED),
            // A reply for another address.
            arp_frame(Operation::Reply, (PEER, PEER_IP), [0; 4]),
        ] {
            let mut dropped = Dropped::default();
            stack.input(probed, &frame, &mut dropped);
            assert!(dropped.0.is_empty(), "{:?}", dropped.0);
            assert_eq!(configuration(stack).dhcp, Some(DhcpState::Probing));
        }
        let (probes, _) = probed_from(stack, probed);
        assert_eq!(probes.len(), 2);
        assert_eq!(configuration(stack).ipv4, Some(leased_ipv4()));
    });
}

#[test]
fn a_lease_that_ends_while_its_address_is_probed_grants_nothing() {
    leasing!(|stack| {
        let xid = requesting(stack, at(0));
        let mut ack = Reply::new(MessageType::Ack, xid);
        ack.options = with(
            ack_options(),
            option::LEASE_TIME,
            Some(1_u32.to_be_bytes().to_vec()),
        );
        give(stack, at(0), &ack.frame());
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Probing));
        // The first probe goes within a second, the second at least a
        // second later: the lease's end at 1 s comes first and starts
        // discovery over.
        first_probe(stack);
        let due = stack.poll_deadline().unwrap();
        assert_eq!(due, secs(1));
        assert_discover(&one(stack, due));
        assert_eq!(configuration(stack).ipv4, None);
        assert_eq!(configuration(stack).lease, None);
    });
}

#[test]
fn a_new_link_while_probing_starts_discovery_over() {
    leasing!(|stack| {
        let xid = acked(stack, at(0));
        let probed = first_probe(stack);
        link_down(stack);
        quiet(stack, probed);
        link_up(stack, 2);
        let restarted = probed.saturating_add(Duration::from_millis(1));
        let sent = dhcp_turn(stack, restarted);
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_ne!(assert_discover(&sent[0]), xid);
        assert!(probes_at(stack, restarted).is_empty());
        assert_eq!(configuration(stack).ipv4, None);
    });
}

#[test]
fn a_nak_to_init_reboot_starts_discovery_without_the_address() {
    leasing!(|stack| {
        bound(stack, at(0));
        link_down(stack);
        quiet(stack, secs(10));
        link_up(stack, 2);
        let sent = one(stack, secs(11));
        give(
            stack,
            secs(11),
            &Reply::new(MessageType::Nak, sent.xid()).frame(),
        );
        assert_eq!(configuration(stack).ipv4, None);
        assert_discover(&one(stack, secs(11)));
    });
}

#[test]
fn an_unanswered_init_reboot_repeats_on_the_discover_schedule_until_the_lease_ends() {
    leasing!(|stack| {
        bound(stack, at(0));
        link_down(stack);
        quiet(stack, secs(10));
        link_up(stack, 2);
        let first = one(stack, secs(11));
        // The DISCOVER's schedule: 250 ms doubling to 10 s, each wait less
        // up to a quarter; the same transaction throughout.
        let mut last = secs(11);
        for nominal in [250, 500, 1000, 2000, 4000, 8000, 10_000, 10_000] {
            let due = stack.poll_deadline().unwrap();
            let wait = due.as_micros() - last.as_micros();
            assert!(
                (nominal * 750..=nominal * 1000).contains(&wait),
                "{wait} µs for {nominal} ms"
            );
            quiet(stack, Instant::from_micros(due.as_micros() - 1));
            let sent = one(stack, due);
            assert_eq!(sent.xid(), first.xid());
            assert_eq!(sent.kind(), MessageType::Request);
            assert_eq!(sent.message().fixed.ciaddr, [0; 4]);
            assert_eq!(
                sent.option(option::REQUESTED_ADDRESS),
                Some(OFFERED.to_vec())
            );
            assert_eq!(sent.option(option::SERVER_IDENTIFIER), None);
            // Unanswered, the client keeps asking, and its address stays in
            // use, RFC 2131, 3.7, never confirmed: still REBOOTING.
            assert_eq!(configuration(stack).dhcp, Some(DhcpState::Rebooting));
            assert_eq!(configuration(stack).ipv4, Some(leased_ipv4()));
            last = due;
        }
        // Its retransmissions go on to the lease's end at 1000 s, where the
        // address goes and discovery starts over.
        let version = stack.address_version();
        let mut now = last;
        while now < secs(1000) {
            now = stack.poll_deadline().unwrap();
            let sent = one(stack, now);
            if now < secs(1000) {
                assert_eq!(sent.kind(), MessageType::Request);
                assert_eq!(configuration(stack).dhcp, Some(DhcpState::Rebooting));
            } else {
                assert_discover(&sent);
            }
        }
        assert_eq!(now, secs(1000));
        assert_eq!(configuration(stack).ipv4, None);
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Selecting));
        assert_ne!(stack.address_version(), version);
    });
}

#[test]
fn an_ack_to_a_late_init_reboot_retransmission_confirms_the_lease() {
    leasing!(|stack| {
        bound(stack, at(0));
        link_down(stack);
        link_up(stack, 2);
        let first = one(stack, secs(11));
        let mut due = secs(11);
        for _ in 0..3 {
            due = stack.poll_deadline().unwrap();
            one(stack, due);
        }
        let version = stack.address_version();
        give(
            stack,
            due,
            &Reply::new(MessageType::Ack, first.xid()).frame(),
        );
        // A lease the client held already goes on without a probe.
        let configured = configuration(stack);
        assert_eq!(configured.dhcp, Some(DhcpState::Bound));
        assert_eq!(configured.ipv4, Some(leased_ipv4()));
        assert_eq!(stack.address_version(), version);
        // It counts from the transaction's first REQUEST.
        assert_eq!(configured.lease.unwrap().obtained, secs(11));
        assert!(probes_at(stack, due).is_empty());
        assert_eq!(stack.poll_deadline(), Some(configured.lease.unwrap().renew));
    });
}

#[test]
fn a_link_back_without_a_lease_discovers_at_once_with_a_new_transaction() {
    leasing!(|stack| {
        link_up(stack, 1);
        let xid = assert_discover(&one(stack, at(0)));
        one(stack, stack.poll_deadline().unwrap());
        link_down(stack);
        quiet(stack, secs(1));
        link_up(stack, 2);
        let sent = one(stack, at(1100));
        assert_ne!(assert_discover(&sent), xid);
        // From the first wait again.
        let wait = stack.poll_deadline().unwrap().as_micros() - at(1100).as_micros();
        assert!((187_500..=250_000).contains(&wait));
    });
}

#[test]
fn the_same_link_seen_again_restarts_nothing() {
    leasing!(|stack| {
        link_up(stack, 1);
        assert_discover(&one(stack, at(0)));
        quiet(stack, at(10));
        quiet(stack, at(20));
    });
}

#[test]
fn the_lease_expires_during_a_link_loss() {
    leasing!(|stack| {
        bound(stack, at(0));
        link_down(stack);
        quiet(stack, secs(999));
        let version = stack.address_version();
        quiet(stack, secs(1000));
        assert_eq!(configuration(stack).ipv4, None);
        assert_ne!(stack.address_version(), version);
        link_up(stack, 2);
        assert_discover(&one(stack, secs(1001)));
    });
}

#[test]
fn foreign_and_malformed_messages_are_counted_and_change_nothing() {
    leasing!(|stack| {
        let xid = requesting(stack, at(0));
        let before = configuration(stack);
        let deadline = stack.poll_deadline();
        let wrong_xid = Reply::new(MessageType::Ack, xid.wrapping_add(1));
        let mut foreign = Reply::new(MessageType::Ack, xid);
        foreign.chaddr = MOVED;
        let mut request = Reply::new(MessageType::Ack, xid);
        request.op = Op::Request;
        let mut no_server = Reply::new(MessageType::Ack, xid);
        no_server.server = None;
        let mut no_mask = Reply::new(MessageType::Ack, xid);
        no_mask.options = with(ack_options(), option::SUBNET_MASK, None);
        let mut gapped_mask = Reply::new(MessageType::Ack, xid);
        gapped_mask.options = with(
            ack_options(),
            option::SUBNET_MASK,
            Some(vec![255, 0, 255, 0]),
        );
        let mut no_time = Reply::new(MessageType::Ack, xid);
        no_time.options = with(ack_options(), option::LEASE_TIME, None);
        let mut zero_time = Reply::new(MessageType::Ack, xid);
        zero_time.options = with(ack_options(), option::LEASE_TIME, Some(vec![0; 4]));
        let mut far_router = Reply::new(MessageType::Ack, xid);
        far_router.options = with(ack_options(), option::ROUTER, Some(vec![10, 0, 0, 1]));
        let mut ragged_dns = Reply::new(MessageType::Ack, xid);
        ragged_dns.options = with(
            ack_options(),
            option::DOMAIN_NAME_SERVER,
            Some(vec![1, 1, 1]),
        );
        let mut broadcast_address = Reply::new(MessageType::Ack, xid);
        broadcast_address.yiaddr = [192, 0, 2, 255];
        let offer = Reply::new(MessageType::Offer, xid);
        let cases: Vec<(Vec<u8>, DhcpDrop)> = vec![
            (wrong_xid.frame(), DhcpDrop::Transaction),
            (foreign.frame(), DhcpDrop::Foreign),
            (request.frame(), DhcpDrop::NotReply),
            (no_server.frame(), DhcpDrop::NoServer),
            (no_mask.frame(), DhcpDrop::Lease(LeaseFault::Mask)),
            (gapped_mask.frame(), DhcpDrop::Lease(LeaseFault::Mask)),
            (no_time.frame(), DhcpDrop::Lease(LeaseFault::Time)),
            (zero_time.frame(), DhcpDrop::Lease(LeaseFault::Time)),
            (far_router.frame(), DhcpDrop::Lease(LeaseFault::Router)),
            (ragged_dns.frame(), DhcpDrop::Lease(LeaseFault::Servers)),
            (
                broadcast_address.frame(),
                DhcpDrop::Lease(LeaseFault::Address),
            ),
            (offer.frame(), DhcpDrop::Unexpected),
        ];
        for (frame, drop) in cases {
            let dropped = give(stack, at(1), &frame);
            assert_eq!(dropped.0, [NetEvent::Dhcp(drop)], "{drop:?}");
            assert_eq!(configuration(stack), before, "{drop:?}");
            assert_eq!(stack.poll_deadline(), deadline);
            quiet(stack, at(1));
        }
        // A message cut short, and one without the magic cookie.
        let mut short = Reply::new(MessageType::Ack, xid).frame();
        let cut = short.len() - 100;
        short.truncate(cut);
        // The IPv4 and UDP lengths still claim the whole: a malformed frame.
        assert_eq!(give(stack, at(1), &short).0, [NetEvent::Malformed]);
        let message = &Reply::new(MessageType::Ack, xid).frame()[42..42 + 200];
        let truncated = udp_from(SERVER, SERVER_IP, OFFERED, (67, 68), message);
        assert_eq!(
            give(stack, at(1), &truncated).0,
            [NetEvent::Dhcp(DhcpDrop::Malformed(
                oer_protocols_network_wire::dhcp::DhcpError::Truncated
            ))]
        );
        // From a port that is no server's.
        let stray = udp_from(SERVER, SERVER_IP, OFFERED, (1067, 68), b"x");
        assert_eq!(give(stack, at(1), &stray).0, [NetEvent::NoSocket]);
        assert_eq!(configuration(stack), before);
    });
}

#[test]
fn an_offer_of_no_host_or_from_no_host_is_refused() {
    leasing!(|stack| {
        link_up(stack, 1);
        let xid = assert_discover(&one(stack, at(0)));
        let mut from_nowhere = Reply::new(MessageType::Offer, xid);
        from_nowhere.from = [0; 4];
        let mut multicast = Reply::new(MessageType::Offer, xid);
        multicast.yiaddr = [224, 0, 0, 1];
        for reply in [from_nowhere, multicast] {
            assert_eq!(
                give(stack, at(10), &reply.frame()).0,
                [NetEvent::Dhcp(DhcpDrop::Offer)]
            );
            quiet(stack, at(10));
        }
    });
}

#[test]
fn a_duplicate_ack_in_bound_is_unexpected() {
    leasing!(|stack| {
        let xid = bound(stack, at(0));
        let lease = configuration(stack);
        assert_eq!(
            give(stack, secs(1), &Reply::new(MessageType::Ack, xid).frame()).0,
            [NetEvent::Dhcp(DhcpDrop::Unexpected)]
        );
        assert_eq!(configuration(stack), lease);
    });
}

fn times_of<B>(stack: &mut Stack<B>, t1: Option<u32>, t2: Option<u32>) -> (u64, u64) {
    let mut options = ack_options();
    if let Some(t1) = t1 {
        options.push((option::RENEWAL_TIME, t1.to_be_bytes().to_vec()));
    }
    if let Some(t2) = t2 {
        options.push((option::REBINDING_TIME, t2.to_be_bytes().to_vec()));
    }
    bound_with(stack, at(0), options);
    let lease = configuration(stack).lease.unwrap();
    (
        lease.renew.as_micros() / 1_000_000,
        lease.rebind.as_micros() / 1_000_000,
    )
}

/// Each case on a brand of its own, as a brand claims once.
macro_rules! times {
    ($t1:expr, $t2:expr) => {{
        brand!(Net);
        let mut times = (0, 0);
        run::<Net>(dhcp_interface(), |stack| times = times_of(stack, $t1, $t2));
        times
    }};
}

fn micros_of<B>(stack: &mut Stack<B>, t1: Option<u32>, t2: Option<u32>) -> (u64, u64) {
    times_of(stack, t1, t2);
    let lease = configuration(stack).lease.unwrap();
    (lease.renew.as_micros(), lease.rebind.as_micros())
}

macro_rules! times_micros {
    ($t1:expr, $t2:expr) => {{
        brand!(Net);
        let mut times = (0, 0);
        run::<Net>(dhcp_interface(), |stack| times = micros_of(stack, $t1, $t2));
        times
    }};
}

#[test]
fn a_lease_takes_the_servers_t1_and_t2_when_they_are_in_order() {
    // A lease of 1000 s.
    assert_eq!(times!(Some(100), Some(200)), (100, 200));
    // T1 alone: T2 three quarters of the way from it to the end.
    assert_eq!(times!(Some(200), None), (200, 800));
    // T2 alone: T1 half the lease, or half T2 when T2 comes first, so T1
    // stays before T2.
    assert_eq!(times!(None, Some(300)), (150, 300));
    assert_eq!(times!(None, Some(900)), (500, 900));
    // Out of order, or past the end: the defaults.
    assert_eq!(times!(Some(900), Some(200)), (500, 875));
    assert_eq!(times!(Some(2000), None), (500, 875));
}

#[test]
fn an_infinite_lease_is_kept_for_its_value() {
    leasing!(|stack| {
        bound_with(
            stack,
            at(0),
            with(ack_options(), option::LEASE_TIME, Some(vec![0xFF; 4])),
        );
        let lease = configuration(stack).lease.unwrap();
        assert_eq!(lease.expires.as_micros(), u64::from(u32::MAX) * 1_000_000);
    });
}

#[test]
fn more_dns_servers_than_a_lease_holds_keep_the_first() {
    leasing!(|stack| {
        let servers = [[1, 1, 1, 1], [8, 8, 8, 8], [9, 9, 9, 9], [4, 4, 4, 4]];
        bound_with(
            stack,
            at(0),
            with(
                ack_options(),
                option::DOMAIN_NAME_SERVER,
                Some(servers.concat()),
            ),
        );
        let lease = configuration(stack).lease.unwrap();
        assert_eq!(lease.dns.as_slice(), &servers[..crate::DNS_SERVERS]);
    });
}

#[test]
fn secs_counts_from_the_start_of_the_acquisition() {
    leasing!(|stack| {
        link_up(stack, 1);
        one(stack, at(0));
        let mut late = None;
        while late.is_none() {
            let due = stack.poll_deadline().unwrap();
            let sent = one(stack, due);
            if due >= secs(3) {
                late = Some((due, sent.message().fixed.secs));
            }
        }
        let (due, seconds) = late.unwrap();
        assert_eq!(u64::from(seconds), due.as_micros() / 1_000_000);
    });
}

#[test]
fn data_leaves_a_neighbor_entry_for_the_renewal() {
    leasing!(|stack| {
        bound(stack, at(0));
        let socket = stack.open(7, best_effort()).unwrap();
        // Two entries: data takes one, the second stays for control.
        let block = filled(stack.pool(), b"one");
        stack.send(secs(400), &socket, peer(), block).unwrap();
        let other = Remote {
            address: [192, 0, 2, 3],
            port: 7,
        };
        let block = filled(stack.pool(), b"two");
        let refused = stack.send(secs(400), &socket, other, block).unwrap_err();
        assert_eq!(refused.reason, SendRefusal::NeighborsFull);
        stack.pool().give(refused.block);
        // The data's neighbor answers, its datagram stays on.
        let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OFFERED);
        stack.input(secs(400), &reply, &mut ());
        // T1: the renewal finds its entry.
        let frames = turn(stack, secs(500), &mut Dropped::default());
        assert!(
            frames.iter().any(|frame| frame[12..14] == [8, 6]),
            "ARP for the server"
        );
        server_answers_arp(stack, secs(500));
        assert_extension(&one(stack, secs(500)), false);
    });
}

#[test]
fn a_static_interface_has_no_client_and_its_port_68_is_a_socket() {
    world!(|stack| {
        assert_eq!(stack.configuration().dhcp, None);
        assert_eq!(stack.configuration().lease, None);
        let socket = stack.open(68, best_effort()).unwrap();
        let datagram = udp_frame(67, 68, b"to a socket");
        stack.input(at(0), &datagram, &mut ());
        assert!(stack.receive(&socket).unwrap().is_some());
    });
}

#[test]
fn a_lost_address_drops_the_echo_replies_owed() {
    leasing!(|stack| {
        bound(stack, at(0));
        let free = stack.pool().free();
        let mut dropped = Dropped::default();
        let request = super::echo::icmp_frame(OFFERED, &super::echo::echo(1, b"ping"));
        stack.input(secs(990), &request, &mut dropped);
        assert!(dropped.0.is_empty());
        assert_eq!(stack.pool().free(), free - 1);
        stack.tick(secs(1000), &mut dropped);
        assert!(
            dropped
                .0
                .contains(&NetEvent::Icmp(crate::IcmpDrop::Readdressed))
        );
        assert_eq!(stack.pool().free(), free);
    });
}

#[test]
fn a_router_that_is_no_host_is_none_and_the_next_one_counts() {
    leasing!(|stack| {
        bound_with(
            stack,
            at(0),
            with(ack_options(), option::ROUTER, Some(vec![0, 0, 0, 0])),
        );
        assert_eq!(configuration(stack).ipv4.unwrap().gateway(), None);
    });
    leasing!(|stack| {
        bound_with(
            stack,
            at(0),
            with(
                ack_options(),
                option::ROUTER,
                Some([[0, 0, 0, 0], SERVER_IP].concat()),
            ),
        );
        assert_eq!(
            configuration(stack).ipv4.unwrap().gateway(),
            Some(SERVER_IP)
        );
    });
}

#[test]
fn a_neighbor_data_holds_takes_no_second_entry_even_once_expired() {
    leasing!(|stack| {
        bound(stack, at(0));
        let socket = stack.open(7, best_effort()).unwrap();
        let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OFFERED);
        let block = filled(stack.pool(), b"one");
        stack.send(secs(10), &socket, peer(), block).unwrap();
        stack.input(secs(10), &reply, &mut ());
        // Its entry expires with the datagram still waiting; a second
        // datagram to the same peer reuses it in place.
        let block = filled(stack.pool(), b"two");
        stack.send(secs(100), &socket, peer(), block).unwrap();
    });
}

// Review of #1187 at cdf1ecc74: its seven counterexamples, as regressions.

#[test]
fn an_echo_s_end_releases_the_neighbors_wait_counter() {
    leasing!(|stack| {
        bound(stack, at(0));
        let now = secs(10);
        let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OFFERED);
        stack.input(now, &reply, &mut ());
        let request = super::echo::icmp_frame(OFFERED, &super::echo::echo(1, b"ping"));
        stack.input(now, &request, &mut ());
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let next = Remote {
            address: [192, 0, 2, 3],
            port: 7,
        };
        assert_eq!(
            stack.declare(now, &socket, next, 16).err(),
            Some(SendRefusal::NeighborsFull)
        );
        let seen = stack.neighbors_changed();
        let demand = stack.poll_demand(now).unwrap();
        assert_eq!(demand.ready.class, Class::Control);
        stack.transmit(now, demand.grant(), unproduced).unwrap();
        assert_ne!(
            stack.neighbors_changed(),
            seen,
            "a held producer learns that a neighbor can be used"
        );
        assert!(stack.declare(now, &socket, next, 16).is_ok());
    });
}

#[test]
fn an_echo_refused_for_good_releases_the_neighbors_wait_counter() {
    leasing!(|stack| {
        bound(stack, at(0));
        let now = secs(10);
        let reply = arp_frame(Operation::Reply, (PEER, PEER_IP), OFFERED);
        stack.input(now, &reply, &mut ());
        let request = super::echo::icmp_frame(OFFERED, &super::echo::echo(1, b"ping"));
        stack.input(now, &request, &mut ());
        let seen = stack.neighbors_changed();
        stack.port_mut().rejects = true;
        let demand = stack.poll_demand(now).unwrap();
        assert!(stack.transmit(now, demand.grant(), unproduced).is_err());
        assert_ne!(stack.neighbors_changed(), seen);
        assert_eq!(stack.poll_demand(now), None);
    });
}

#[test]
fn a_cached_neighbor_data_does_not_hold_takes_the_reserve_like_any_other() {
    leasing!(|stack| {
        bound(stack, at(0));
        let now = secs(10);
        let other_ip = [192, 0, 2, 3];
        for (hardware, ip) in [(PEER, PEER_IP), (MOVED, other_ip)] {
            stack.input(
                now,
                &arp_frame(Operation::Reply, (hardware, ip), OFFERED),
                &mut (),
            );
        }
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let _first = stack.declare(now, &socket, peer(), 16).unwrap();
        let second = stack.declare(
            now,
            &socket,
            Remote {
                address: other_ip,
                port: 7,
            },
            16,
        );
        assert_eq!(second.err(), Some(SendRefusal::NeighborsFull));
        // The entry data holds stays its own, expired too.
        assert!(stack.declare(secs(100), &socket, peer(), 16).is_ok());
    });
}

#[test]
fn warm_data_neighbors_cannot_keep_the_renewal_from_its_server() {
    leasing!(|stack| {
        bound(stack, at(0));
        let now = secs(490);
        let other_ip = [192, 0, 2, 3];
        for (hardware, ip) in [(PEER, PEER_IP), (MOVED, other_ip)] {
            stack.input(
                now,
                &arp_frame(Operation::Reply, (hardware, ip), OFFERED),
                &mut (),
            );
        }
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let _first = stack.declare(now, &socket, peer(), 16).unwrap();
        let _second = stack.declare(
            now,
            &socket,
            Remote {
                address: other_ip,
                port: 7,
            },
            16,
        );
        stack.tick(secs(500), &mut ());
        assert_eq!(stack.configuration().dhcp, Some(DhcpState::Renewing));
        let ready: Vec<_> = stack.demands(secs(500)).collect();
        assert!(
            ready.iter().any(|work| work.ready.class == Class::Control),
            "T1 owes a renewal or its server's ARP: {ready:?}"
        );
    });
}

#[test]
fn an_ack_already_expired_from_its_request_enables_no_address() {
    leasing!(|stack| {
        let xid = requesting(stack, at(0));
        let now = secs(2);
        // A tick comes before intake, as in a service's turn.
        stack.tick(now, &mut ());
        let mut ack = Reply::new(MessageType::Ack, xid);
        ack.options = with(
            ack_options(),
            option::LEASE_TIME,
            Some(1_u32.to_be_bytes().to_vec()),
        );
        assert_eq!(
            give(stack, now, &ack.frame()).0,
            [NetEvent::Dhcp(DhcpDrop::Lease(LeaseFault::Expired))]
        );
        assert!(stack.configuration().ipv4.is_none());
        assert_eq!(stack.configuration().dhcp, Some(DhcpState::Requesting));
    });
}

#[test]
fn a_late_expired_ack_publishes_no_user_data_before_the_next_tick() {
    leasing!(|stack| {
        let xid = requesting(stack, at(0));
        let now = secs(2);
        stack.tick(now, &mut ());
        let mut ack = Reply::new(MessageType::Ack, xid);
        ack.options = with(
            ack_options(),
            option::LEASE_TIME,
            Some(1_u32.to_be_bytes().to_vec()),
        );
        give(stack, now, &ack.frame());
        let socket = stack.open(7, best_effort()).unwrap();
        let block = filled(stack.pool(), b"expired");
        let to = Remote {
            address: [255; 4],
            port: 7,
        };
        let refused = stack.send(now, &socket, to, block).unwrap_err();
        assert_eq!(refused.reason, SendRefusal::NoAddress);
        stack.pool().give(refused.block);
    });
}

#[test]
fn a_renewal_ack_already_past_its_end_keeps_the_lease_to_its_own_end() {
    leasing!(|stack| {
        bound(stack, at(0));
        // A rebind REQUEST at T2, its ACK of a 100 s lease 200 s late.
        let xid = assert_extension(&one(stack, secs(875)), true);
        let mut ack = Reply::new(MessageType::Ack, xid);
        ack.options = with(
            ack_options(),
            option::LEASE_TIME,
            Some(100_u32.to_be_bytes().to_vec()),
        );
        assert_eq!(
            give(stack, secs(990), &ack.frame()).0,
            [NetEvent::Dhcp(DhcpDrop::Lease(LeaseFault::Expired))]
        );
        assert_eq!(configuration(stack).lease.unwrap().expires, secs(1000));
        // At the old lease's end the address goes, as without the ACK.
        one(stack, secs(1000));
        assert_eq!(configuration(stack).ipv4, None);
    });
}

#[test]
fn a_lease_at_its_exact_end_carries_no_send() {
    leasing!(|stack| {
        bound(stack, at(0));
        let socket = stack.open(7, best_effort()).unwrap();
        let block = filled(stack.pool(), b"late");
        // No tick at 1000 s yet: the send itself meets the end.
        let refused = stack.send(secs(1000), &socket, peer(), block).unwrap_err();
        assert_eq!(refused.reason, SendRefusal::NoAddress);
        stack.pool().give(refused.block);
        let block = filled(stack.pool(), b"in time");
        assert!(stack.send(at(999_999), &socket, peer(), block).is_ok());
    });
}

#[test]
fn an_ack_with_only_an_early_t2_still_renews_before_it_rebinds() {
    leasing!(|stack| {
        bound_with(
            stack,
            at(0),
            with(
                ack_options(),
                option::REBINDING_TIME,
                Some(300_u32.to_be_bytes().to_vec()),
            ),
        );
        let lease = stack.configuration().lease.unwrap();
        assert!(lease.renew < lease.rebind, "{lease:?}");
        turn(stack, secs(150), &mut Dropped::default());
        assert_eq!(configuration(stack).dhcp, Some(DhcpState::Renewing));
    });
}

#[test]
fn t1_and_t2_stay_in_order_at_their_bounds() {
    // T2 of one second: T1 half of it.
    assert_eq!(times_micros!(None, Some(1)), (500_000, 1_000_000));
    // T2 at half the lease: T1 half of T2, not equal to it.
    assert_eq!(times_micros!(None, Some(500)), (250_000_000, 500_000_000));
    // A zero T2 cannot follow any T1: the defaults.
    assert_eq!(times_micros!(None, Some(0)), (500_000_000, 875_000_000));
    // T1 a second before the end: T2 three quarters of that second on.
    assert_eq!(times_micros!(Some(999), None), (999_000_000, 999_750_000));
    // T2 equal to T1, or to the end: the defaults.
    assert_eq!(
        times_micros!(Some(300), Some(300)),
        (500_000_000, 875_000_000)
    );
    assert_eq!(
        times_micros!(Some(300), Some(1000)),
        (500_000_000, 875_000_000)
    );
}
