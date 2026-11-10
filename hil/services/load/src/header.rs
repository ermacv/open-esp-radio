//! The header of a load datagram, big-endian, at the start of its UDP
//! payload; the rest of the payload is filler, [`FILLER`] in every octet.
//! The receivers read the header only and never check the filler.
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
/// The octet in every place of a load datagram behind its header: both
/// stacks' generators and the host's sender put the same content on air.
pub const FILLER: u8 = 0x5a;
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
    /// The header's octets, as they lead a datagram's payload.
    pub fn to_bytes(&self) -> [u8; LENGTH] {
        let mut flags = 0;
        if self.echo_request {
            flags |= ECHO_REQUEST;
        }
        if self.echo_reply {
            flags |= ECHO_REPLY;
        }
        // Each field in its place, written once: no pass over the octets.
        let [m0, m1, m2, m3] = MAGIC;
        let [r0, r1, r2, r3] = self.run.to_be_bytes();
        let [q0, q1, q2, q3, q4, q5, q6, q7] = self.sequence.to_be_bytes();
        let [t0, t1, t2, t3, t4, t5, t6, t7] = self.sent_us.to_be_bytes();
        [
            m0, m1, m2, m3, flags, 0, 0, 0, r0, r1, r2, r3, q0, q1, q2, q3, q4, q5, q6, q7, t0, t1,
            t2, t3, t4, t5, t6, t7,
        ]
    }

    /// Write the header into the first [`LENGTH`] bytes of `payload`; `None`
    /// when it is shorter.
    pub fn write(&self, payload: &mut [u8]) -> Option<()> {
        let header = payload.get_mut(..LENGTH)?;
        for (slot, byte) in header.iter_mut().zip(self.to_bytes()) {
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
