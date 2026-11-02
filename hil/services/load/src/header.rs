//! The header of a load datagram, big-endian, at the start of its UDP
//! payload; the rest of the payload is filler.
//!
//! | Bytes | Field |
//! | --- | --- |
//! | 0..4 | `OERL` |
//! | 4 | flags: bit 0 asks for an echo, bit 1 marks an echo reply |
//! | 5..8 | zero |
//! | 8..12 | run |
//! | 12..20 | sequence |
//! | 20..28 | the sender's timestamp, microseconds of its own clock |

/// The header's length; a load datagram is at least this long.
pub const LENGTH: usize = 28;
const MAGIC: [u8; 4] = *b"OERL";
const ECHO_REQUEST: u8 = 1;
const ECHO_REPLY: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    pub run: u32,
    pub sequence: u64,
    pub sent_us: u64,
    /// The receiver sends the datagram back to its source.
    pub echo_request: bool,
    /// This datagram is an echo of the receiver's own datagram.
    pub echo_reply: bool,
}

impl Header {
    /// Write the header into the first [`LENGTH`] bytes of `payload`; `None`
    /// when it is shorter.
    pub fn write(&self, payload: &mut [u8]) -> Option<()> {
        let header = payload.get_mut(..LENGTH)?;
        let mut flags = 0;
        if self.echo_request {
            flags |= ECHO_REQUEST;
        }
        if self.echo_reply {
            flags |= ECHO_REPLY;
        }
        let bytes = MAGIC
            .into_iter()
            .chain([flags, 0, 0, 0])
            .chain(self.run.to_be_bytes())
            .chain(self.sequence.to_be_bytes())
            .chain(self.sent_us.to_be_bytes());
        for (slot, byte) in header.iter_mut().zip(bytes) {
            *slot = byte;
        }
        Some(())
    }

    /// The header at the start of `payload`; `None` when it is too short or
    /// holds no load header.
    pub fn read(payload: &[u8]) -> Option<Self> {
        let field = |range: core::ops::Range<usize>| payload.get(range);
        if field(0..4)? != MAGIC {
            return None;
        }
        let flags = *payload.get(4)?;
        if flags & !(ECHO_REQUEST | ECHO_REPLY) != 0 || field(5..8)? != [0, 0, 0] {
            return None;
        }
        Some(Self {
            run: u32::from_be_bytes(field(8..12)?.try_into().ok()?),
            sequence: u64::from_be_bytes(field(12..20)?.try_into().ok()?),
            sent_us: u64::from_be_bytes(field(20..28)?.try_into().ok()?),
            echo_request: flags & ECHO_REQUEST != 0,
            echo_reply: flags & ECHO_REPLY != 0,
        })
    }
}
