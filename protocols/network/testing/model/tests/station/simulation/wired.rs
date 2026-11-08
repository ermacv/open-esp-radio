//! The wired side behind the access point: a router that is the DHCP server
//! (RFC 2131) and a peer host. Both answer ARP for their addresses; the peer
//! sends ICMP echo requests (RFC 792) and UDP datagrams to the station and
//! keeps what the station sends it.

use std::collections::VecDeque;

use oer_portable_time::Instant;
use oer_portable_wire::Writer;
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use oer_protocols_network_wire::dhcp::{self, DhcpMessage, Fixed, Flags, MessageType, Op, option};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::icmp::{self, Echo, PROTOCOL_ICMP};
use oer_protocols_network_wire::ip::{DsField, Ipv4Fields, Ipv4Header};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UdpHeader};

/// The router: the gateway and the DHCP server.
pub const ROUTER_MAC: [u8; 6] = [0x02, 0, 0, 0, 0, 0x01];
pub const ROUTER_IP: [u8; 4] = [192, 0, 2, 1];
/// The peer host on the same network.
pub const PEER_MAC: [u8; 6] = [0x02, 0, 0, 0, 0, 0x02];
pub const PEER_IP: [u8; 4] = [192, 0, 2, 2];
/// The address the server leases first.
pub const LEASED_IP: [u8; 4] = [192, 0, 2, 100];
pub const MASK: [u8; 4] = [255, 255, 255, 0];
const BROADCAST_IP: [u8; 4] = [255; 4];

/// An Ethernet frame's fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Msdu {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ether_type: u16,
    pub payload: Vec<u8>,
}

/// A client message the server took.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DhcpSeen {
    pub at: Instant,
    pub kind: MessageType,
    pub xid: u32,
    pub ciaddr: [u8; 4],
    pub requested: Option<[u8; 4]>,
    pub server: Option<[u8; 4]>,
    /// The IP destination: the broadcast address or the server's.
    pub broadcast: bool,
}

/// How the server answers a REQUEST.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Answer {
    Ack,
    Nak,
    /// No answer at all.
    Silent,
}

/// A DHCP server of one address at a time.
pub struct DhcpServer {
    /// What a lease lasts, in seconds; T1 and T2 when the server names them.
    pub lease_secs: u32,
    pub renewal_secs: Option<u32>,
    pub rebinding_secs: Option<u32>,
    /// The address an OFFER names.
    pub address: [u8; 4],
    /// The answer to the next REQUEST, by what it is.
    pub answer: Box<dyn FnMut(&DhcpSeen) -> Answer>,
    /// Whether the server offers at all.
    pub offers: bool,
    /// Every client message, in order, and the server's answers.
    pub seen: Vec<DhcpSeen>,
    pub answered: Vec<(Instant, MessageType)>,
    /// The address the server last acknowledged.
    pub bound: Option<[u8; 4]>,
}

impl Default for DhcpServer {
    fn default() -> Self {
        Self {
            lease_secs: 3600,
            renewal_secs: None,
            rebinding_secs: None,
            address: LEASED_IP,
            answer: Box::new(|_| Answer::Ack),
            offers: true,
            seen: Vec::new(),
            answered: Vec::new(),
            bound: None,
        }
    }
}

impl DhcpServer {
    /// The kinds of the client's messages, in order.
    pub fn kinds(&self) -> Vec<MessageType> {
        self.seen.iter().map(|seen| seen.kind).collect()
    }
}

/// A UDP datagram the peer received.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Received {
    pub at: Instant,
    pub source: ([u8; 4], u16),
    pub port: u16,
    pub payload: Vec<u8>,
}

/// An echo reply the peer received.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EchoReply {
    pub at: Instant,
    pub source: [u8; 4],
    pub identifier: u16,
    pub sequence: u16,
    pub data: Vec<u8>,
}

/// The router and the peer.
#[derive(Default)]
pub struct Wired {
    pub dhcp: DhcpServer,
    /// The station's hardware address, once it spoke.
    pub station: Option<[u8; 6]>,
    pub udp: Vec<Received>,
    pub echo_replies: Vec<EchoReply>,
    /// ARP requests answered, by target.
    pub arp_answered: Vec<[u8; 4]>,
    /// Frames the station sent that neither host took.
    pub ignored: Vec<Msdu>,
    outgoing: VecDeque<Msdu>,
    identification: u16,
}

impl Wired {
    pub fn new(dhcp: DhcpServer) -> Self {
        Self {
            dhcp,
            ..Self::default()
        }
    }

    /// What the hosts have for the station.
    pub fn take_outgoing(&mut self) -> VecDeque<Msdu> {
        core::mem::take(&mut self.outgoing)
    }

    /// The station's address, as the server last acknowledged it.
    pub fn station_ip(&self) -> Option<[u8; 4]> {
        self.dhcp.bound
    }

    /// The peer sends a UDP datagram of `payload` from `source_port` to the
    /// station's `port`.
    pub fn send_udp(&mut self, source_port: u16, port: u16, payload: &[u8]) {
        let (Some(station), Some(address)) = (self.station, self.station_ip()) else {
            panic!("the station has no address to send to");
        };
        let packet = self.udp_packet(PEER_IP, address, source_port, port, payload);
        self.outgoing.push_back(Msdu {
            destination: station,
            source: PEER_MAC,
            ether_type: EtherType::IPV4.value(),
            payload: packet,
        });
    }

    /// The peer sends an echo request to the station.
    pub fn ping(&mut self, identifier: u16, sequence: u16, data: &[u8]) {
        let (Some(station), Some(address)) = (self.station, self.station_ip()) else {
            panic!("the station has no address to ping");
        };
        let echo = Echo {
            kind: icmp::MessageType::Echo,
            code: 0,
            checksum: 0,
            identifier,
            sequence,
            data,
        };
        let mut message = echo.header(icmp::MessageType::Echo).to_vec();
        message.extend_from_slice(data);
        let packet = self.ip_packet(PEER_IP, address, PROTOCOL_ICMP, &message);
        self.outgoing.push_back(Msdu {
            destination: station,
            source: PEER_MAC,
            ether_type: EtherType::IPV4.value(),
            payload: packet,
        });
    }

    /// An MSDU from the station.
    pub fn from_station(&mut self, msdu: Msdu, now: Instant) {
        self.station = Some(msdu.source);
        let taken = match EtherType::new(msdu.ether_type) {
            EtherType::ARP => self.arp(&msdu),
            EtherType::IPV4 => self.ipv4(&msdu, now),
            _ => false,
        };
        if !taken {
            self.ignored.push(msdu);
        }
    }

    fn arp(&mut self, msdu: &Msdu) -> bool {
        let Ok(Some(request)) = Arp::parse(&msdu.payload) else {
            return false;
        };
        if request.operation != Operation::Request {
            return false;
        }
        let hardware = match request.target_protocol {
            ROUTER_IP => ROUTER_MAC,
            PEER_IP => PEER_MAC,
            _ => return false,
        };
        let mut packet = vec![0; arp::PACKET_LEN];
        Arp {
            operation: Operation::Reply,
            sender_hardware: hardware,
            sender_protocol: request.target_protocol,
            target_hardware: request.sender_hardware,
            target_protocol: request.sender_protocol,
        }
        .write(&mut packet)
        .expect("an ARP packet");
        self.arp_answered.push(request.target_protocol);
        self.outgoing.push_back(Msdu {
            destination: request.sender_hardware,
            source: hardware,
            ether_type: EtherType::ARP.value(),
            payload: packet,
        });
        true
    }

    fn ipv4(&mut self, msdu: &Msdu, now: Instant) -> bool {
        let Ok(ip) = Ipv4Header::parse(&msdu.payload) else {
            return false;
        };
        let addresses = ip.addresses();
        let source: [u8; 4] = addresses[..4].try_into().expect("four octets");
        let destination: [u8; 4] = addresses[4..].try_into().expect("four octets");
        match ip.protocol() {
            PROTOCOL_UDP => {
                let Ok((udp, data)) = UdpHeader::parse(ip.payload()) else {
                    return false;
                };
                assert!(
                    udp.checksum == 0
                        || Checksum::new()
                            .with(&ipv4_pseudo_header(
                                source,
                                destination,
                                PROTOCOL_UDP,
                                udp.length
                            ))
                            .with(ip.payload())
                            .verifies(),
                    "the station's UDP checksum verifies"
                );
                if udp.ports.destination == dhcp::SERVER_PORT {
                    return self.dhcp(msdu.source, destination, data, now);
                }
                if destination == PEER_IP {
                    self.udp.push(Received {
                        at: now,
                        source: (source, udp.ports.source),
                        port: udp.ports.destination,
                        payload: data.to_vec(),
                    });
                    return true;
                }
                false
            }
            PROTOCOL_ICMP if destination == PEER_IP => {
                assert!(
                    Checksum::new().with(ip.payload()).verifies(),
                    "the station's ICMP checksum verifies"
                );
                let Ok(echo) = Echo::parse(ip.payload()) else {
                    return false;
                };
                if echo.kind != icmp::MessageType::EchoReply {
                    return false;
                }
                self.echo_replies.push(EchoReply {
                    at: now,
                    source,
                    identifier: echo.identifier,
                    sequence: echo.sequence,
                    data: echo.data.to_vec(),
                });
                true
            }
            _ => false,
        }
    }

    fn dhcp(&mut self, station: [u8; 6], destination: [u8; 4], data: &[u8], now: Instant) -> bool {
        let Ok(message) = DhcpMessage::parse(data) else {
            return false;
        };
        let Some(kind) = message.message_type() else {
            return false;
        };
        let address = |code| {
            message
                .option(code)
                .and_then(|value| <[u8; 4]>::try_from(value).ok())
        };
        let seen = DhcpSeen {
            at: now,
            kind,
            xid: message.fixed.xid,
            ciaddr: message.fixed.ciaddr,
            requested: address(option::REQUESTED_ADDRESS),
            server: address(option::SERVER_IDENTIFIER),
            broadcast: destination == BROADCAST_IP,
        };
        self.dhcp.seen.push(seen);
        let reply = match kind {
            MessageType::Discover if self.dhcp.offers => Some(MessageType::Offer),
            MessageType::Request => match (self.dhcp.answer)(&seen) {
                Answer::Ack => Some(MessageType::Ack),
                Answer::Nak => Some(MessageType::Nak),
                Answer::Silent => None,
            },
            _ => None,
        };
        let Some(reply) = reply else {
            return true;
        };
        // An ACK grants what the client asks for: the address it holds or
        // requests; an OFFER names the server's.
        let granted = match reply {
            MessageType::Ack => seen
                .requested
                .or((seen.ciaddr != [0; 4]).then_some(seen.ciaddr))
                .unwrap_or(self.dhcp.address),
            _ => self.dhcp.address,
        };
        let message = self.dhcp_reply(reply, &message.fixed, granted);
        // RFC 2131, 4.1: a NAK goes to the broadcast address; the rest to the
        // client's address, which takes unicast before it is configured.
        let destination = if reply == MessageType::Nak {
            BROADCAST_IP
        } else {
            granted
        };
        let packet = self.udp_packet(
            ROUTER_IP,
            destination,
            dhcp::SERVER_PORT,
            dhcp::CLIENT_PORT,
            &message,
        );
        self.dhcp.answered.push((now, reply));
        match reply {
            MessageType::Ack => self.dhcp.bound = Some(granted),
            MessageType::Nak => self.dhcp.bound = None,
            _ => {}
        }
        self.outgoing.push_back(Msdu {
            destination: station,
            source: ROUTER_MAC,
            ether_type: EtherType::IPV4.value(),
            payload: packet,
        });
        true
    }

    /// A BOOTREPLY of `kind` to the client's `request` (RFC 2131, Table 3).
    fn dhcp_reply(&self, kind: MessageType, request: &Fixed, granted: [u8; 4]) -> Vec<u8> {
        let mut message = vec![0; 512];
        let mut writer = Writer::new(&mut message);
        Fixed {
            op: Op::Reply,
            htype: 1,
            hlen: 6,
            hops: 0,
            xid: request.xid,
            secs: 0,
            flags: Flags::from_bytes([0; 2]),
            ciaddr: request.ciaddr,
            yiaddr: if kind == MessageType::Nak {
                [0; 4]
            } else {
                granted
            },
            siaddr: [0; 4],
            giaddr: [0; 4],
            chaddr: request.chaddr,
        }
        .write(&mut writer)
        .expect("the fixed fields");
        let write = |writer: &mut Writer<'_>, code, data: &[u8]| {
            dhcp::write_option(writer, code, data).expect("an option");
        };
        write(&mut writer, option::MESSAGE_TYPE, &[kind.encoding()]);
        write(&mut writer, option::SERVER_IDENTIFIER, &ROUTER_IP);
        if kind != MessageType::Nak {
            write(&mut writer, option::SUBNET_MASK, &MASK);
            write(&mut writer, option::ROUTER, &ROUTER_IP);
            write(
                &mut writer,
                option::LEASE_TIME,
                &self.dhcp.lease_secs.to_be_bytes(),
            );
            if let Some(renewal) = self.dhcp.renewal_secs {
                write(&mut writer, option::RENEWAL_TIME, &renewal.to_be_bytes());
            }
            if let Some(rebinding) = self.dhcp.rebinding_secs {
                write(
                    &mut writer,
                    option::REBINDING_TIME,
                    &rebinding.to_be_bytes(),
                );
            }
        }
        dhcp::write_end(&mut writer).expect("the end option");
        let length = writer.written();
        message.truncate(length);
        message
    }

    fn udp_packet(
        &mut self,
        source: [u8; 4],
        destination: [u8; 4],
        source_port: u16,
        port: u16,
        payload: &[u8],
    ) -> Vec<u8> {
        let length = u16::try_from(8 + payload.len()).expect("a UDP length");
        let unfilled = UdpHeader {
            ports: Ports {
                source: source_port,
                destination: port,
            },
            length,
            checksum: 0,
        };
        let checksum = udp_field(
            Checksum::new()
                .with(&ipv4_pseudo_header(
                    source,
                    destination,
                    PROTOCOL_UDP,
                    length,
                ))
                .with(&unfilled.to_bytes())
                .with(payload)
                .finish(),
        );
        let mut datagram = UdpHeader {
            checksum,
            ..unfilled
        }
        .to_bytes()
        .to_vec();
        datagram.extend_from_slice(payload);
        self.ip_packet(source, destination, PROTOCOL_UDP, &datagram)
    }

    fn ip_packet(
        &mut self,
        source: [u8; 4],
        destination: [u8; 4],
        protocol: u8,
        payload: &[u8],
    ) -> Vec<u8> {
        self.identification = self.identification.wrapping_add(1);
        let mut packet = Ipv4Fields {
            services: DsField::from_bytes([0]),
            total_length: u16::try_from(20 + payload.len()).expect("an IPv4 length"),
            identification: self.identification,
            time_to_live: 64,
            protocol,
            source,
            destination,
        }
        .to_bytes()
        .to_vec();
        packet.extend_from_slice(payload);
        packet
    }
}

/// An Ethernet frame of `msdu`, as a test reads one.
#[allow(dead_code, reason = "a case's reading")]
pub fn ethernet(msdu: &Msdu) -> Vec<u8> {
    let mut frame = EthernetHeader {
        destination: msdu.destination,
        source: msdu.source,
        ether_type: EtherType::new(msdu.ether_type),
    }
    .to_bytes()
    .to_vec();
    frame.extend_from_slice(&msdu.payload);
    frame
}
