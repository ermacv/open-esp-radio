//! DHCP messages over BOOTP: the fixed fields, the magic cookie and the
//! options that follow it.
//!
//! SOURCE: RFC 2131, 2 (Figure 1, Format of a DHCP message; Table 1,
//! Description of fields; Figure 2, Format of the 'flags' field) and 4.1
//! (ports); RFC 2132, 2 (option field format, magic cookie). Options are read
//! from the 'options' field only: options overloaded into 'sname' and 'file'
//! (RFC 2131, 4.1) are not interpreted.

use oer_portable_wire::octet_code;
use oer_portable_wire::{Error, Reader, Writer};

use crate::address::Ipv4Address;

/// RFC 2131, 4.1: messages from a client go to the 'DHCP server' port.
pub const SERVER_PORT: u16 = 67;
/// RFC 2131, 4.1: messages from a server go to the 'DHCP client' port.
pub const CLIENT_PORT: u16 = 68;
/// RFC 2131, 2, Figure 1: 'op' through 'file'.
pub const FIXED_LEN: usize = 236;
/// RFC 2132, 2: the four octets 99.130.83.99 that begin the options.
pub const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];
/// The fixed fields and the magic cookie: everything before the options.
pub const HEADER_LEN: usize = FIXED_LEN + MAGIC_COOKIE.len();
/// RFC 2131, 2, Table 1: 'chaddr' is sixteen octets.
const CHADDR_LEN: usize = 16;
/// RFC 2131, 2, Table 1: 'sname' and 'file'.
const SNAME_LEN: usize = 64;
const FILE_LEN: usize = 128;

oer_portable_wire::bitfield! {
    /// 'op': the message op code.
    /// SOURCE: RFC 2131, 2, Table 1.
    pub enum Op: u8 {
        /// BOOTREQUEST.
        Request = 1,
        /// BOOTREPLY.
        Reply = 2,
    }
}

oer_portable_wire::bitfield! {
    /// The DHCP Message Type option's value.
    /// SOURCE: RFC 2132, 9.6 (DHCP Message Type).
    pub enum MessageType: u8 {
        Discover = 1,
        Offer = 2,
        Request = 3,
        Decline = 4,
        Ack = 5,
        Nak = 6,
        Release = 7,
        Inform = 8,
    }
}

oer_portable_wire::bitfield! {
    /// The 'flags' field.
    /// SOURCE: RFC 2131, 2, Figure 2 (Format of the 'flags' field).
    pub struct Flags[2] (big) {
        /// MBZ: zero from a client, ignored by servers.
        pub reserved, with_reserved: u16 = 0..15;
        /// B: ask that replies be broadcast.
        pub broadcast, with_broadcast: bool = 15..16;
    }
}

oer_portable_wire::bitfield! {
    /// The fields before 'flags'.
    /// SOURCE: RFC 2131, 2, Figure 1 (Format of a DHCP message).
    struct Opening[10] (big) {
        op, with_op: {u8} = 72..80;
        htype, with_htype: {u8} = 64..72;
        hlen, with_hlen: {u8} = 56..64;
        hops, with_hops: {u8} = 48..56;
        xid, with_xid: {u32} = 16..48;
        secs, with_secs: {u16} = 0..16;
    }
}

octet_code! { "oer_protocols_network_wire::dhcp";
    /// The code of an option: any octet, some of them named.
    OptionCode {
        /// SOURCE: RFC 2132, 3.1 (Pad Option).
        PAD = 0,
        /// SOURCE: RFC 2132, 3.2 (End Option).
        END = 255,
        /// SOURCE: RFC 2132, 3.3 (Subnet Mask).
        SUBNET_MASK = 1,
        /// SOURCE: RFC 2132, 3.5 (Router Option).
        ROUTER = 3,
        /// SOURCE: RFC 2132, 3.8 (Domain Name Server Option).
        DOMAIN_NAME_SERVER = 6,
        /// SOURCE: RFC 2132, 9.1 (Requested IP Address).
        REQUESTED_ADDRESS = 50,
        /// SOURCE: RFC 2132, 9.2 (IP Address Lease Time).
        LEASE_TIME = 51,
        /// SOURCE: RFC 2132, 9.6 (DHCP Message Type).
        MESSAGE_TYPE = 53,
        /// SOURCE: RFC 2132, 9.7 (Server Identifier).
        SERVER_IDENTIFIER = 54,
        /// SOURCE: RFC 2132, 9.8 (Parameter Request List).
        PARAMETER_REQUEST_LIST = 55,
        /// SOURCE: RFC 2132, 9.10 (Maximum DHCP Message Size).
        MAXIMUM_MESSAGE_SIZE = 57,
        /// SOURCE: RFC 2132, 9.11 (Renewal (T1) Time Value).
        RENEWAL_TIME = 58,
        /// SOURCE: RFC 2132, 9.12 (Rebinding (T2) Time Value).
        REBINDING_TIME = 59,
        /// SOURCE: RFC 2132, 9.14 (Client-identifier).
        CLIENT_IDENTIFIER = 61,
    }
}

octet_code! { "oer_protocols_network_wire::dhcp";
    /// A hardware type of one octet: DHCP's 'htype', and the value of ARP's
    /// wider field.
    ///
    /// SOURCE: IANA Address Resolution Protocol Parameters (Hardware Types);
    /// RFC 2131, 2 ('htype').
    HardwareType {
        /// Ethernet.
        ETHERNET = 1,
    }
}

oer_portable_wire::bitfield! {
    /// The value of an option that counts seconds.
    /// SOURCE: RFC 2132, 9.2 (IP Address Lease Time), 9.11 (Renewal (T1) Time
    /// Value) and 9.12 (Rebinding (T2) Time Value).
    pub struct Seconds[4] (big) {
        pub value, with_value: {u32} = 0..32;
    }
}

oer_portable_wire::bitfield! {
    /// The value of the Maximum DHCP Message Size option.
    /// SOURCE: RFC 2132, 9.10 (Maximum DHCP Message Size).
    pub struct MessageSize[2] (big) {
        pub octets, with_octets: {u16} = 0..16;
    }
}

/// A message that cannot be DHCP.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DhcpError {
    /// Fewer octets than the fixed fields and the magic cookie.
    Truncated,
    /// The options do not begin with the magic cookie: BOOTP, or not this.
    Cookie,
    /// An option's length runs past the options field.
    Option,
}

/// The fixed fields of a message, RFC 2131, 2, Table 1. 'sname' and 'file'
/// are not kept: a written message carries them zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fixed {
    /// 'op'.
    pub op: Op,
    /// 'htype': the ARP hardware type, 1 for Ethernet.
    pub htype: HardwareType,
    /// 'hlen'.
    pub hlen: u8,
    /// 'hops'.
    pub hops: u8,
    /// 'xid': the transaction.
    pub xid: u32,
    /// 'secs': seconds since the client began this acquisition or renewal.
    pub secs: u16,
    /// 'flags'.
    pub flags: Flags,
    /// 'ciaddr': the client's address, in BOUND, RENEW and REBINDING.
    pub ciaddr: Ipv4Address,
    /// 'yiaddr': the address offered or leased.
    pub yiaddr: Ipv4Address,
    /// 'siaddr'.
    pub siaddr: Ipv4Address,
    /// 'giaddr': a relay agent's address.
    pub giaddr: Ipv4Address,
    /// 'chaddr'.
    pub chaddr: [u8; CHADDR_LEN],
}

impl Fixed {
    /// Write the fixed fields, zero 'sname' and 'file', and the magic cookie.
    ///
    /// # Errors
    /// [`Error`] when `writer` has fewer than [`HEADER_LEN`] octets left.
    pub fn write(&self, writer: &mut Writer<'_>) -> Result<(), Error> {
        if writer.remaining() < HEADER_LEN {
            return Err(Error::Truncated {
                required: HEADER_LEN,
                remaining: writer.remaining(),
            });
        }
        let opening = Opening::from_bytes([0; 10])
            .with_op(self.op.encoding())
            .with_htype(self.htype.value())
            .with_hlen(self.hlen)
            .with_hops(self.hops)
            .with_xid(self.xid)
            .with_secs(self.secs);
        writer.write(&opening.to_bytes())?;
        writer.write(&self.flags.to_bytes())?;
        writer.write(self.ciaddr.as_octets())?;
        writer.write(self.yiaddr.as_octets())?;
        writer.write(self.siaddr.as_octets())?;
        writer.write(self.giaddr.as_octets())?;
        writer.write(&self.chaddr)?;
        writer.write(&[0; SNAME_LEN])?;
        writer.write(&[0; FILE_LEN])?;
        writer.write(&MAGIC_COOKIE)
    }
}

/// A checked message over the caller's octets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DhcpMessage<'a> {
    pub fixed: Fixed,
    /// The 'options' field after the magic cookie.
    options: &'a [u8],
}

impl<'a> DhcpMessage<'a> {
    /// The message at the front of `bytes`: the fixed fields and the magic
    /// cookie must be there, and every option within the field. Options are
    /// checked here, so [`Self::option`] never meets a malformed one.
    ///
    /// # Errors
    /// [`DhcpError`].
    pub fn parse(bytes: &'a [u8]) -> Result<Self, DhcpError> {
        let mut reader = Reader::new(bytes);
        let fixed = reader
            .read::<FIXED_LEN>()
            .map_err(|_| DhcpError::Truncated)?;
        let cookie = reader.read::<4>().map_err(|_| DhcpError::Truncated)?;
        if cookie != MAGIC_COOKIE {
            return Err(DhcpError::Cookie);
        }
        let mut fields = Reader::new(&fixed);
        let opening = Opening::from_bytes(fields.read().map_err(|_| DhcpError::Truncated)?);
        let flags = Flags::from_bytes(fields.read().map_err(|_| DhcpError::Truncated)?);
        let mut address = || Ipv4Address::read(&mut fields).map_err(|_| DhcpError::Truncated);
        let (ciaddr, yiaddr, siaddr, giaddr) = (address()?, address()?, address()?, address()?);
        let chaddr = fields.read().map_err(|_| DhcpError::Truncated)?;
        let message = Self {
            fixed: Fixed {
                op: Op::from_encoding(opening.op()),
                htype: HardwareType::new(opening.htype()),
                hlen: opening.hlen(),
                hops: opening.hops(),
                xid: opening.xid(),
                secs: opening.secs(),
                flags,
                ciaddr,
                yiaddr,
                siaddr,
                giaddr,
                chaddr,
            },
            options: reader.remaining(),
        };
        for option in message.options() {
            option?;
        }
        Ok(message)
    }

    /// Every option up to the End option or the field's end, Pad skipped:
    /// `(code, data)`.
    pub fn options(&self) -> Options<'a> {
        Options {
            reader: Reader::new(self.options),
            done: false,
        }
    }

    /// The data of the first option `code`; RFC 2131, 4.1: an option may
    /// appear once unless its own document says otherwise.
    pub fn option(&self, code: OptionCode) -> Option<&'a [u8]> {
        self.options()
            .filter_map(Result::ok)
            .find(|(found, _)| *found == code)
            .map(|(_, data)| data)
    }

    /// The DHCP Message Type, RFC 2132, 9.6: `None` without a one-octet
    /// option.
    pub fn message_type(&self) -> Option<MessageType> {
        match self.option(OptionCode::MESSAGE_TYPE)? {
            [kind] => Some(MessageType::from_encoding(*kind)),
            _ => None,
        }
    }
}

/// The options of a message, in order.
#[derive(Clone, Debug)]
pub struct Options<'a> {
    reader: Reader<'a>,
    done: bool,
}

impl<'a> Iterator for Options<'a> {
    type Item = Result<(OptionCode, &'a [u8]), DhcpError>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.done {
            let Ok(code) = self.reader.u8() else {
                self.done = true;
                return None;
            };
            match OptionCode::new(code) {
                OptionCode::PAD => {}
                OptionCode::END => self.done = true,
                code => {
                    let data = self
                        .reader
                        .u8()
                        .and_then(|length| self.reader.take(usize::from(length)));
                    return Some(match data {
                        Ok(data) => Ok((code, data)),
                        Err(_) => {
                            self.done = true;
                            Err(DhcpError::Option)
                        }
                    });
                }
            }
        }
        None
    }
}

/// Write one option, RFC 2132, 2: code, length, data.
///
/// # Errors
/// [`Error::ValueOutOfRange`] for data longer than 255 octets, or a
/// truncation; nothing is written then.
pub fn write_option(writer: &mut Writer<'_>, code: OptionCode, data: &[u8]) -> Result<(), Error> {
    let length = u8::try_from(data.len()).map_err(|_| Error::ValueOutOfRange)?;
    let total = data.len().saturating_add(2);
    if writer.remaining() < total {
        return Err(Error::Truncated {
            required: total,
            remaining: writer.remaining(),
        });
    }
    writer.write(&[code.value(), length])?;
    writer.write(data)
}

/// Write the End option, RFC 2132, 3.2, which RFC 2131, 4.1 puts last.
///
/// # Errors
/// [`Error`] when no octet is left.
pub fn write_end(writer: &mut Writer<'_>) -> Result<(), Error> {
    writer.u8(OptionCode::END.value())
}

#[cfg(test)]
mod tests;
