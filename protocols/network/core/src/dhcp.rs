//! The DHCPv4 client of the one interface.
//!
//! SOURCE: RFC 2131, 3.1, 3.2, 4.1 and 4.4 (client behaviour, Figure 5's
//! states, Table 5's fields and options); RFC 2132 for the options. Ported
//! from Xarxa fbebde5 (`src/iface/dhcpv4.rs`): a DISCOVER's first repeat after
//! 250 ms, doubling to 10 s in one transaction; REQUEST retries after 5 s,
//! doubling every two, five in all; renewals and rebinds after half the time
//! left, no less than 60 s apart. Unlike the fork: each wait is jittered down
//! by up to a quarter, as RFC 2131, 4.1 asks for a randomized backoff; a lease
//! counts from its REQUEST's first acceptance by the port, RFC 2131, 4.4.1 and
//! 4.4.5; a renewal goes to the server identifier, RFC 2131, 4.1; an ACK
//! without a lease time or subnet mask is refused, not given a default; a link
//! that comes back with a lease runs INIT-REBOOT, RFC 2131, 3.2 and 4.4.2.

use oer_portable_time::{Duration, Instant};
use oer_portable_wire::{Error, Writer};
use oer_protocols_network_wire::dhcp::{
    self, DhcpMessage, Fixed, Flags, HEADER_LEN, MessageType, Op, option,
};

use crate::interface::{BROADCAST, Ipv4, UNSPECIFIED, is_unicast};

/// The first wait for an OFFER: a first DISCOVER can reach an access point
/// before its port opens, which a station cannot see (up to 119 ms after the
/// handshake's last message on the stand, Xarxa fbebde5).
const DISCOVER_FIRST: Duration = Duration::from_millis(250);
/// The longest wait for an OFFER.
const DISCOVER_MOST: Duration = Duration::from_secs(10);
/// The first wait for an ACK to the REQUEST that answers an OFFER; it doubles
/// every two tries.
const REQUEST_FIRST: Duration = Duration::from_secs(5);
/// REQUESTs to an OFFER before discovery starts again.
const REQUEST_TRIES: u8 = 5;
/// REQUESTs of an INIT-REBOOT before the client keeps its lease, RFC 2131,
/// 3.2 (3). They start at [`DISCOVER_FIRST`]: the link just came up.
const REBOOT_TRIES: u8 = 5;
/// RFC 2131, 4.4.5: the shortest wait between renewal or rebinding REQUESTs.
const RENEW_LEAST: Duration = Duration::from_secs(60);
/// RFC 2132, 9.8: the subnet mask, router and domain name servers.
const PARAMETERS: [u8; 3] = [
    option::SUBNET_MASK,
    option::ROUTER,
    option::DOMAIN_NAME_SERVER,
];
/// RFC 2132, 9.14 and the ARP hardware type 1: Ethernet.
const HARDWARE_ETHERNET: u8 = 1;
/// The largest IPv4 header, RFC 791, 3.1 (IHL of 15 words), and UDP's.
const HEADERS_MOST: u16 = 60 + 8;
/// The domain name servers a lease keeps.
pub const DNS_SERVERS: usize = 3;
/// RFC 2131, 3.3: a lease time of all ones is infinite. It is kept as its
/// value in seconds, some 136 years.
const INFINITE: u32 = u32::MAX;

/// The client's state, RFC 2131, 4.4, Figure 5.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DhcpState {
    /// INIT and SELECTING: a DISCOVER is owed or waits for an OFFER.
    Selecting,
    /// A REQUEST answers an OFFER.
    Requesting,
    Bound,
    /// Past T1: REQUESTs go to the server that leased the address.
    Renewing,
    /// Past T2: REQUESTs go to every server.
    Rebinding,
    /// INIT-REBOOT and REBOOTING: the link came back, and a REQUEST asks to
    /// keep the lease; the address stays in use meanwhile, RFC 2131, 3.7.
    Rebooting,
}

/// The domain name servers of a lease, in the server's order.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Servers {
    addresses: [[u8; 4]; DNS_SERVERS],
    count: u8,
}

impl Servers {
    pub fn as_slice(&self) -> &[[u8; 4]] {
        self.addresses
            .get(..usize::from(self.count))
            .unwrap_or_default()
    }

    fn push(&mut self, address: [u8; 4]) {
        if let Some(slot) = self.addresses.get_mut(usize::from(self.count)) {
            *slot = address;
            self.count = self.count.saturating_add(1);
        }
    }
}

/// A lease, as a server granted it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lease {
    /// The address, its prefix and the router, RFC 2132, 3.3 and 3.5.
    pub ipv4: Ipv4,
    /// RFC 2132, 3.8: the unicast ones, at most [`DNS_SERVERS`].
    pub dns: Servers,
    /// RFC 2132, 9.7: where renewals go.
    pub server: [u8; 4],
    /// When the REQUEST that won it was first accepted: the times count from
    /// here.
    pub obtained: Instant,
    /// T1, T2 and the end, RFC 2131, 4.4.5.
    pub renew: Instant,
    pub rebind: Instant,
    pub expires: Instant,
}

/// Why a server's message was dropped. The client goes on as it was.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DhcpDrop {
    /// Not a DHCP message, RFC 2131, 2.
    Malformed(dhcp::DhcpError),
    /// 'op' is not BOOTREPLY.
    NotReply,
    /// 'htype' and 'hlen' are not Ethernet's.
    Hardware,
    /// 'chaddr' is another client's.
    Foreign,
    /// No DHCP Message Type, RFC 2132, 9.6.
    NoType,
    /// 'xid' is not the open transaction's, RFC 2131, 4.4.1 and 4.4.5.
    Transaction,
    /// No server identifier, which RFC 2131, Table 3 makes a server send.
    NoServer,
    /// A message this state does not take, RFC 2131, Figure 5.
    Unexpected,
    /// An OFFER of an address that is no host's, or from no host.
    Offer,
    /// An ACK whose lease cannot be used.
    Lease(LeaseFault),
}

/// What makes an ACK's lease unusable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaseFault {
    /// 'yiaddr' is no host's, or its network's own or broadcast address.
    Address,
    /// No subnet mask of four contiguous octets, RFC 2132, 3.3.
    Mask,
    /// No lease time of four octets, RFC 2132, 9.2, or a zero one.
    Time,
    /// A router option not of whole addresses, or a first router off the
    /// network, RFC 2132, 3.5.
    Router,
    /// A domain name server option not of whole addresses, RFC 2132, 3.8.
    Servers,
}

/// What a message or a time did to the interface's address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Change {
    /// A lease is held, new or extended.
    Leased(Ipv4),
    /// The lease ended: expired, or refused by a NAK.
    Lost,
}

/// Where a message goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Destination {
    Broadcast,
    /// The leasing server, through the neighbor table.
    Server([u8; 4]),
}

/// A message the client owes the port, as values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Outgoing {
    kind: MessageType,
    xid: u32,
    secs: u16,
    ciaddr: [u8; 4],
    requested: Option<[u8; 4]>,
    server: Option<[u8; 4]>,
    hardware: [u8; 6],
    largest: u16,
    pub(crate) source: [u8; 4],
    pub(crate) destination: [u8; 4],
}

impl Outgoing {
    /// The message's octets.
    pub(crate) const fn length(&self) -> usize {
        /// Message type, client identifier, maximum size, parameters, end.
        const ALWAYS: usize = HEADER_LEN + 3 + 9 + 4 + (2 + PARAMETERS.len()) + 1;
        /// An address option.
        const ADDRESS: usize = 6;
        match (self.requested.is_some(), self.server.is_some()) {
            (false, false) => ALWAYS,
            (true, false) | (false, true) => ALWAYS + ADDRESS,
            (true, true) => ALWAYS + 2 * ADDRESS,
        }
    }

    /// Write it at the front of `writer`, RFC 2131, Table 5.
    pub(crate) fn write(&self, writer: &mut Writer<'_>) -> Result<(), Error> {
        let [h0, h1, h2, h3, h4, h5] = self.hardware;
        Fixed {
            op: Op::Request,
            htype: HARDWARE_ETHERNET,
            hlen: 6,
            hops: 0,
            xid: self.xid,
            secs: self.secs,
            // RFC 2131, 4.1: this client takes unicast before it has an
            // address, so it clears the BROADCAST bit.
            flags: Flags::from_bytes([0; 2]),
            ciaddr: self.ciaddr,
            yiaddr: UNSPECIFIED,
            siaddr: UNSPECIFIED,
            giaddr: UNSPECIFIED,
            chaddr: [h0, h1, h2, h3, h4, h5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        }
        .write(writer)?;
        dhcp::write_option(writer, option::MESSAGE_TYPE, &[self.kind.encoding()])?;
        dhcp::write_option(
            writer,
            option::CLIENT_IDENTIFIER,
            &[HARDWARE_ETHERNET, h0, h1, h2, h3, h4, h5],
        )?;
        if let Some(requested) = self.requested {
            dhcp::write_option(writer, option::REQUESTED_ADDRESS, &requested)?;
        }
        if let Some(server) = self.server {
            dhcp::write_option(writer, option::SERVER_IDENTIFIER, &server)?;
        }
        dhcp::write_option(
            writer,
            option::MAXIMUM_MESSAGE_SIZE,
            &self.largest.to_be_bytes(),
        )?;
        dhcp::write_option(writer, option::PARAMETER_REQUEST_LIST, &PARAMETERS)?;
        dhcp::write_end(writer)
    }
}

/// SplitMix64: the client's transaction identifiers and jitter. Any seed
/// works, zero too.
#[derive(Clone, Copy, Debug)]
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `wait` less up to a quarter of it.
    fn jitter(&mut self, wait: Duration) -> Duration {
        let quarter = wait.as_micros() / 4;
        let less = self
            .next()
            .checked_rem(quarter.saturating_add(1))
            .unwrap_or(0);
        Duration::from_micros(wait.as_micros().saturating_sub(less))
    }
}

#[derive(Clone, Copy, Debug)]
enum Phase {
    Selecting {
        /// When the DISCOVER is repeated; `None` while one is owed.
        next: Option<Instant>,
        wait: Duration,
    },
    Requesting {
        next: Option<Instant>,
        tries: u8,
        server: [u8; 4],
        offered: [u8; 4],
    },
    Bound,
    Renewing {
        next: Option<Instant>,
    },
    Rebinding {
        next: Option<Instant>,
    },
    Rebooting {
        next: Option<Instant>,
        wait: Duration,
        tries: u8,
    },
}

/// The client.
#[derive(Debug)]
pub(crate) struct Client {
    hardware: [u8; 6],
    largest: u16,
    random: Random,
    phase: Phase,
    lease: Option<Lease>,
    xid: u32,
    /// The serial of the message owed, which a grant names.
    owed: Option<u64>,
    /// When the port first took this transaction's REQUEST.
    sent: Option<Instant>,
    /// When this acquisition or renewal began, for 'secs'.
    began: Instant,
    /// The epoch of the link that is up.
    link: Option<u64>,
}

impl Client {
    pub(crate) fn new(hardware: [u8; 6], mtu: u16, seed: u64) -> Self {
        let mut random = Random(seed);
        let xid = xid(&mut random);
        Self {
            hardware,
            // RFC 2132, 9.10: what one IPv4 datagram of the MTU carries
            // with the largest headers, and no less than 576.
            largest: mtu.saturating_sub(HEADERS_MOST).max(576),
            random,
            phase: Phase::Selecting {
                next: None,
                wait: DISCOVER_FIRST,
            },
            lease: None,
            xid,
            owed: None,
            sent: None,
            began: Instant::from_micros(0),
            link: None,
        }
    }

    pub(crate) const fn lease(&self) -> Option<&Lease> {
        self.lease.as_ref()
    }

    pub(crate) const fn state(&self) -> DhcpState {
        match self.phase {
            Phase::Selecting { .. } => DhcpState::Selecting,
            Phase::Requesting { .. } => DhcpState::Requesting,
            Phase::Bound => DhcpState::Bound,
            Phase::Renewing { .. } => DhcpState::Renewing,
            Phase::Rebinding { .. } => DhcpState::Rebinding,
            Phase::Rebooting { .. } => DhcpState::Rebooting,
        }
    }

    const fn up(&self) -> bool {
        self.link.is_some()
    }

    fn owe(&mut self, serial: &mut u64) {
        *serial = serial.wrapping_add(1);
        self.owed = Some(*serial);
    }

    fn new_transaction(&mut self, now: Instant) {
        self.xid = xid(&mut self.random);
        self.sent = None;
        self.began = now;
    }

    /// INIT: a new transaction, its DISCOVER owed now.
    fn restart(&mut self, now: Instant, serial: &mut u64) {
        self.new_transaction(now);
        self.phase = Phase::Selecting {
            next: None,
            wait: DISCOVER_FIRST,
        };
        self.owe(serial);
    }

    /// The link is up, at `epoch`. A new link is a new network as far as
    /// this client knows, RFC 2131, 3.7: with a lease it asks to keep it
    /// (INIT-REBOOT, 4.4.2), and without one it starts discovery over at once,
    /// even mid-backoff: a message spent on the old link never reached this
    /// one.
    pub(crate) fn link_up(&mut self, now: Instant, epoch: u64, serial: &mut u64) {
        if self.link == Some(epoch) {
            return;
        }
        self.link = Some(epoch);
        if self.lease.is_some() {
            self.new_transaction(now);
            self.phase = Phase::Rebooting {
                next: None,
                wait: DISCOVER_FIRST,
                tries: 0,
            };
            self.owe(serial);
        } else {
            self.restart(now, serial);
        }
    }

    /// The link is down: nothing is sent until it is up; the lease's times
    /// go on.
    pub(crate) const fn link_down(&mut self) {
        self.link = None;
    }

    /// Advance to `now`: expire the lease, enter RENEWING or REBINDING, and
    /// owe what a retransmission time asks for.
    pub(crate) fn tick(&mut self, now: Instant, serial: &mut u64) -> Option<Change> {
        let mut change = None;
        // Each pass moves the phase forward or stops: Rebooting → Bound →
        // Renewing → Rebinding at most.
        for _ in 0..4 {
            if let Some(lease) = self.lease
                && now >= lease.expires
            {
                // RFC 2131, 4.4.5: the client MUST stop using the address.
                self.lease = None;
                self.restart(now, serial);
                change = Some(Change::Lost);
            }
            if !self.step(now, serial) {
                break;
            }
        }
        change
    }

    /// One transition `now` asks for; whether one happened.
    fn step(&mut self, now: Instant, serial: &mut u64) -> bool {
        let due = |next: Option<Instant>| next.is_some_and(|next| now >= next);
        match self.phase {
            Phase::Selecting { next, .. } if due(next) => {
                // A repeat keeps its transaction: a slow OFFER answers it as
                // well as the last DISCOVER.
                self.owe(serial);
                self.set_next(None);
                true
            }
            Phase::Requesting { next, tries, .. } if due(next) => {
                if tries >= REQUEST_TRIES {
                    self.restart(now, serial);
                } else {
                    self.owe(serial);
                    self.set_next(None);
                }
                true
            }
            Phase::Rebooting { next, tries, .. } if due(next) => {
                if tries >= REBOOT_TRIES {
                    // RFC 2131, 3.2 (3): no answer, so the client MAY use the
                    // lease for the rest of its time.
                    self.owed = None;
                    self.phase = Phase::Bound;
                } else {
                    self.owe(serial);
                    self.set_next(None);
                }
                true
            }
            Phase::Bound | Phase::Renewing { .. } => {
                let Some(lease) = self.lease else {
                    return false;
                };
                if now >= lease.rebind {
                    self.new_transaction(now);
                    self.phase = Phase::Rebinding { next: None };
                    self.owe(serial);
                    true
                } else if matches!(self.phase, Phase::Bound) && now >= lease.renew {
                    self.new_transaction(now);
                    self.phase = Phase::Renewing { next: None };
                    self.owe(serial);
                    true
                } else if let Phase::Renewing { next } = self.phase
                    && due(next)
                {
                    self.sent = None;
                    self.xid = xid(&mut self.random);
                    self.owe(serial);
                    self.set_next(None);
                    true
                } else {
                    false
                }
            }
            Phase::Rebinding { next } if due(next) => {
                self.sent = None;
                self.xid = xid(&mut self.random);
                self.owe(serial);
                self.set_next(None);
                true
            }
            _ => false,
        }
    }

    const fn set_next(&mut self, at: Option<Instant>) {
        match &mut self.phase {
            Phase::Selecting { next, .. }
            | Phase::Requesting { next, .. }
            | Phase::Renewing { next }
            | Phase::Rebinding { next }
            | Phase::Rebooting { next, .. } => *next = at,
            Phase::Bound => {}
        }
    }

    /// When [`Self::tick`] has work.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        let next = match self.phase {
            Phase::Selecting { next, .. }
            | Phase::Requesting { next, .. }
            | Phase::Renewing { next }
            | Phase::Rebinding { next }
            | Phase::Rebooting { next, .. } => next,
            Phase::Bound => None,
        };
        let lease = self.lease.map(|lease| {
            let step = match self.phase {
                Phase::Bound => Some(lease.renew),
                Phase::Renewing { .. } => Some(lease.rebind),
                _ => None,
            };
            step.map_or(lease.expires, |step| step.min(lease.expires))
        });
        [next, lease].into_iter().flatten().min()
    }

    /// The serial of the message owed, while the link is up to carry it.
    pub(crate) fn owed(&self) -> Option<u64> {
        self.owed.filter(|_| self.up())
    }

    /// Where the owed message goes.
    pub(crate) fn destination(&self) -> Destination {
        match (self.phase, self.lease) {
            (Phase::Renewing { .. }, Some(lease)) => Destination::Server(lease.server),
            _ => Destination::Broadcast,
        }
    }

    /// The message of `serial`, while it is owed, RFC 2131, Table 5.
    pub(crate) fn message(&self, now: Instant, serial: u64) -> Option<Outgoing> {
        if self.owed() != Some(serial) {
            return None;
        }
        let address = self.lease.map(|lease| lease.ipv4.address());
        let (kind, ciaddr, requested, server) = match self.phase {
            Phase::Selecting { .. } => (MessageType::Discover, None, None, None),
            Phase::Requesting {
                server, offered, ..
            } => (MessageType::Request, None, Some(offered), Some(server)),
            Phase::Renewing { .. } | Phase::Rebinding { .. } => {
                (MessageType::Request, address, None, None)
            }
            // RFC 2131, 3.2 (1): 'ciaddr' MUST NOT be filled in.
            Phase::Rebooting { .. } => (MessageType::Request, None, address, None),
            Phase::Bound => return None,
        };
        let secs = u16::try_from(now.saturating_duration_since(self.began).as_micros() / 1_000_000)
            .unwrap_or(u16::MAX);
        let destination = match self.destination() {
            Destination::Broadcast => BROADCAST,
            Destination::Server(server) => server,
        };
        Some(Outgoing {
            kind,
            xid: self.xid,
            secs,
            ciaddr: ciaddr.unwrap_or(UNSPECIFIED),
            requested,
            server,
            hardware: self.hardware,
            largest: self.largest,
            // RFC 2131, 4.1: before it has its address, a client sends
            // from 0.
            source: ciaddr.unwrap_or(UNSPECIFIED),
            destination,
        })
    }

    /// The port took the message of `serial`, or refused it for good: its
    /// retransmission time starts now.
    pub(crate) fn sent(&mut self, now: Instant, serial: u64) {
        if self.owed != Some(serial) {
            return;
        }
        self.owed = None;
        let random = &mut self.random;
        let lease = self.lease;
        match &mut self.phase {
            Phase::Selecting { next, wait } => {
                *next = Some(now.saturating_add(random.jitter(*wait)));
                *wait = Duration::from_micros(
                    wait.as_micros()
                        .saturating_mul(2)
                        .min(DISCOVER_MOST.as_micros()),
                );
            }
            Phase::Requesting { next, tries, .. } => {
                let doubled = REQUEST_FIRST
                    .as_micros()
                    .checked_shl(u32::from(*tries / 2))
                    .unwrap_or(u64::MAX);
                *next = Some(now.saturating_add(random.jitter(Duration::from_micros(doubled))));
                *tries = tries.saturating_add(1);
                self.sent.get_or_insert(now);
            }
            Phase::Rebooting { next, wait, tries } => {
                *next = Some(now.saturating_add(random.jitter(*wait)));
                *wait = Duration::from_micros(wait.as_micros().saturating_mul(2));
                *tries = tries.saturating_add(1);
                self.sent.get_or_insert(now);
            }
            // RFC 2131, 4.4.5: half the time left to T2, or to the end, no
            // less than 60 s; a renewal never waits past T2.
            Phase::Renewing { next } => {
                let left = lease.map_or(Duration::from_micros(0), |lease| {
                    lease.rebind.saturating_duration_since(now)
                });
                let wait = half(left).max(RENEW_LEAST).min(left);
                *next = Some(now.saturating_add(wait));
                self.sent.get_or_insert(now);
            }
            Phase::Rebinding { next } => {
                let left = lease.map_or(Duration::from_micros(0), |lease| {
                    lease.expires.saturating_duration_since(now)
                });
                *next = Some(now.saturating_add(half(left).max(RENEW_LEAST)));
                self.sent.get_or_insert(now);
            }
            Phase::Bound => {}
        }
    }

    /// A server's message from IPv4 `source`, RFC 2131, 4.4.
    ///
    /// # Errors
    /// [`DhcpDrop`]: the client goes on as it was.
    pub(crate) fn input(
        &mut self,
        now: Instant,
        source: [u8; 4],
        bytes: &[u8],
        serial: &mut u64,
    ) -> Result<Option<Change>, DhcpDrop> {
        let message = DhcpMessage::parse(bytes).map_err(DhcpDrop::Malformed)?;
        let fixed = message.fixed;
        if fixed.op != Op::Reply {
            return Err(DhcpDrop::NotReply);
        }
        if fixed.htype != HARDWARE_ETHERNET || fixed.hlen != 6 {
            return Err(DhcpDrop::Hardware);
        }
        if fixed.chaddr.get(..6) != Some(&self.hardware[..]) {
            return Err(DhcpDrop::Foreign);
        }
        let kind = message.message_type().ok_or(DhcpDrop::NoType)?;
        if fixed.xid != self.xid {
            return Err(DhcpDrop::Transaction);
        }
        let server =
            address_of(message.option(option::SERVER_IDENTIFIER)).ok_or(DhcpDrop::NoServer)?;
        match (self.phase, kind) {
            (Phase::Selecting { .. }, MessageType::Offer) => {
                // The first OFFER is taken, RFC 2131, 4.4.1.
                if !is_unicast(fixed.yiaddr) || !is_unicast(source) {
                    return Err(DhcpDrop::Offer);
                }
                self.phase = Phase::Requesting {
                    next: None,
                    tries: 0,
                    server,
                    offered: fixed.yiaddr,
                };
                self.owe(serial);
                Ok(None)
            }
            (
                Phase::Requesting { .. }
                | Phase::Renewing { .. }
                | Phase::Rebinding { .. }
                | Phase::Rebooting { .. },
                MessageType::Ack,
            ) => {
                let obtained = self.sent.unwrap_or(now);
                let lease = lease(&message, server, obtained).map_err(DhcpDrop::Lease)?;
                self.lease = Some(lease);
                self.phase = Phase::Bound;
                self.owed = None;
                Ok(Some(Change::Leased(lease.ipv4)))
            }
            (
                Phase::Requesting { .. }
                | Phase::Renewing { .. }
                | Phase::Rebinding { .. }
                | Phase::Rebooting { .. },
                MessageType::Nak,
            ) => {
                // RFC 2131, Figure 5 and 3.2 (3): back to INIT.
                let held = self.lease.take().is_some();
                self.restart(now, serial);
                Ok(held.then_some(Change::Lost))
            }
            _ => Err(DhcpDrop::Unexpected),
        }
    }
}

/// The lease an ACK grants, counted from `obtained`.
fn lease(
    message: &DhcpMessage<'_>,
    server: [u8; 4],
    obtained: Instant,
) -> Result<Lease, LeaseFault> {
    let address = message.fixed.yiaddr;
    if !is_unicast(address) {
        return Err(LeaseFault::Address);
    }
    let mask = address_of(message.option(option::SUBNET_MASK)).ok_or(LeaseFault::Mask)?;
    let mask = u32::from_be_bytes(mask);
    let prefix = u8::try_from(mask.leading_ones()).map_err(|_| LeaseFault::Mask)?;
    if mask.checked_shl(u32::from(prefix)).unwrap_or(0) != 0 {
        return Err(LeaseFault::Mask);
    }
    // The first router that is a host, in the server's order of preference,
    // RFC 2132, 3.5: one of 0.0.0.0 is none (Xarxa fbebde5).
    let router = match message.option(option::ROUTER) {
        None => None,
        Some(routers) => {
            let mut first = None;
            let mut rest = routers;
            while !rest.is_empty() {
                let (router, after) = rest.split_first_chunk::<4>().ok_or(LeaseFault::Router)?;
                first = first.or(Some(*router).filter(|router| is_unicast(*router)));
                rest = after;
            }
            if routers.is_empty() {
                return Err(LeaseFault::Router);
            }
            first
        }
    };
    let ipv4 = Ipv4::new(address, prefix, router).map_err(|_| LeaseFault::Router)?;
    if ipv4.is_network(address) || ipv4.is_directed_broadcast(address) {
        return Err(LeaseFault::Address);
    }
    let mut dns = Servers::default();
    if let Some(servers) = message.option(option::DOMAIN_NAME_SERVER) {
        if servers.is_empty() {
            return Err(LeaseFault::Servers);
        }
        let mut rest = servers;
        while !rest.is_empty() {
            let (server, after) = rest.split_first_chunk::<4>().ok_or(LeaseFault::Servers)?;
            // Some servers name 0.0.0.0 for an empty slot: only hosts are
            // kept.
            if is_unicast(*server) {
                dns.push(*server);
            }
            rest = after;
        }
    }
    let seconds = |code| {
        message
            .option(code)
            .and_then(|data| data.first_chunk::<4>().filter(|_| data.len() == 4))
            .map(|data| u32::from_be_bytes(*data))
    };
    let length = seconds(option::LEASE_TIME)
        .filter(|seconds| *seconds != 0)
        .ok_or(LeaseFault::Time)?;
    let length = if length == INFINITE {
        Duration::from_secs(INFINITE)
    } else {
        Duration::from_secs(length)
    };
    let (renew, rebind) = times(
        length,
        seconds(option::RENEWAL_TIME).map(Duration::from_secs),
        seconds(option::REBINDING_TIME).map(Duration::from_secs),
    );
    Ok(Lease {
        ipv4,
        dns,
        server,
        obtained,
        renew: obtained.saturating_add(renew),
        rebind: obtained.saturating_add(rebind),
        expires: obtained.saturating_add(length),
    })
}

/// T1 and T2 of a lease of `length`, RFC 2131, 4.4.5: the server's when T1 <
/// T2 < the lease; with T1 alone, T2 three quarters of the way from it to the
/// end; with T2 alone, T1 the lesser of half the lease and T2; else the
/// defaults, half and seven eighths (Xarxa fbebde5).
fn times(length: Duration, t1: Option<Duration>, t2: Option<Duration>) -> (Duration, Duration) {
    let (total, half_lease) = (length.as_micros(), length.as_micros() / 2);
    match (t1, t2) {
        (Some(t1), Some(t2)) if t1 < t2 && t2 < length => (t1, t2),
        (Some(t1), None) if t1 < length => {
            let gap = total.saturating_sub(t1.as_micros());
            (
                t1,
                Duration::from_micros(t1.as_micros().saturating_add((gap / 4).saturating_mul(3))),
            )
        }
        (None, Some(t2)) if t2 < length => {
            (Duration::from_micros(half_lease.min(t2.as_micros())), t2)
        }
        _ => (
            Duration::from_micros(half_lease),
            Duration::from_micros((total / 8).saturating_mul(7)),
        ),
    }
}

/// A transaction identifier: the high half of the next draw.
fn xid(random: &mut Random) -> u32 {
    u32::try_from(random.next() >> 32).unwrap_or(0)
}

fn half(duration: Duration) -> Duration {
    Duration::from_micros(duration.as_micros() / 2)
}

/// One IPv4 address, an option of exactly four octets.
fn address_of(data: Option<&[u8]>) -> Option<[u8; 4]> {
    match data? {
        [a, b, c, d] => Some([*a, *b, *c, *d]),
        _ => None,
    }
}
