//! The experiment's schedule and the bytes of every frame it transmits.
//!
//! Each subject frame (beacon, probe response, action) carries the caller's
//! value K in octets 24..32 of its MPDU, the Timestamp field of a beacon and
//! of a probe response (IEEE Std 802.11-2020 9.3.3.2, 9.3.3.10, 9.4.1.10)
//! and, for the action frame, the first eight body octets (Category and the
//! following action octets, 9.3.3.13): the action frame is the control
//! whose subtype has no Timestamp field. Every frame also carries a tag, a
//! vendor-specific element (9.4.2.25) with the round, the frame's kind and
//! arm and a copy of K, so a capture identifies it without the device's
//! records. The blocker is a group-addressed data frame from the AP whose
//! 1500-octet body begins with the same tag.
//!
//! The functions write the MPDU only; the hardware appends the FCS.

/// The subject frames' kinds, in schedule order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Beacon,
    ProbeResponse,
    Action,
}

impl Kind {
    pub(crate) const fn code(self) -> u8 {
        match self {
            Self::Beacon => b'b',
            Self::ProbeResponse => b'p',
            Self::Action => b'a',
        }
    }
}

/// The tag's kind code of the blocker.
pub(crate) const BLOCKER_CODE: u8 = b'x';

/// Whether a blocker is on the air ahead of the subject frame when it is
/// published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Arm {
    Undelayed,
    Delayed,
}

impl Arm {
    pub(crate) const fn code(self) -> u8 {
        match self {
            Self::Undelayed => b'u',
            Self::Delayed => b'd',
        }
    }
}

/// Rounds of each kind; consecutive rounds of one kind alternate the arm.
pub(crate) const ROUNDS_PER_KIND: u16 = 32;
const KINDS: [Kind; 3] = [Kind::Beacon, Kind::ProbeResponse, Kind::Action];
/// Every round of the run.
pub(crate) const ROUNDS: u16 = ROUNDS_PER_KIND * KINDS.len() as u16;

/// The kind and arm of round `n`: each kind's rounds in one block, the arm
/// alternating from undelayed.
pub(crate) fn round(n: u16) -> Option<(Kind, Arm)> {
    let block = n.checked_div(ROUNDS_PER_KIND)?;
    let kind = KINDS.get(usize::from(block)).copied()?;
    let arm = if n.is_multiple_of(2) {
        Arm::Undelayed
    } else {
        Arm::Delayed
    };
    Some((kind, arm))
}

/// K of round `n`: every octet in 0x11..=0xd0, so none is zero and none is
/// the element identifier 0xdd, and every octet changes from one round to
/// the next. The high word is never zero: K lies above 2^60, far from any
/// counter of the run.
pub(crate) fn k_of(n: u16) -> u64 {
    let mut k = 0u64;
    let mut octet = 0u32;
    while octet < 8 {
        let step = u32::from(n)
            .wrapping_mul(octet.wrapping_mul(2).wrapping_add(3))
            .wrapping_add(octet.wrapping_mul(7));
        let value = 0x11u32.wrapping_add(step % 0xc0);
        k |= u64::from(value)
            .checked_shl(octet.wrapping_mul(8))
            .unwrap_or(0);
        octet = octet.wrapping_add(1);
    }
    k
}

/// The tag element: identifier, length, the locally administered OUI
/// 02:4f:45, the magic "STMP", the version, the kind and arm codes, the
/// round, the run identifier and K.
pub(crate) const TAG_LENGTH: usize = 26;
const TAG_BODY_LENGTH: u8 = 24;
const TAG_HEAD: [u8; 9] = [
    0xdd,
    TAG_BODY_LENGTH,
    0x02,
    0x4f,
    0x45,
    b'S',
    b'T',
    b'M',
    b'P',
];
const TAG_VERSION: u8 = 1;

/// The beacon's SSID.
pub(crate) const SSID: &[u8] = b"oer-stamp";
/// The beacon interval, in TU.
const BEACON_INTERVAL_TU: u16 = 100;
/// Capability Information: ESS (9.4.1.4).
const CAPABILITY: u16 = 0x0001;
/// Duration of an individually addressed management frame at 1 Mb/s
/// awaiting an ACK: SIFS 10 us plus a 14-octet ACK at 1 Mb/s with the long
/// preamble, 192 + 112 us (IEEE Std 802.11-2020 15.3.3, 10.6.6.5).
const UNICAST_DURATION_US: u16 = 314;
/// The blocker's body: the run's 12.4-ms frame at 1 Mb/s.
pub(crate) const BLOCKER_BODY: usize = 1500;
const HEADER: usize = 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FrameTooLong;

/// A cursor over the caller's buffer.
struct Writer<'a> {
    buffer: &'a mut [u8],
    at: usize,
}

impl<'a> Writer<'a> {
    fn new(buffer: &'a mut [u8]) -> Self {
        Self { buffer, at: 0 }
    }

    fn bytes(&mut self, bytes: &[u8]) -> Result<(), FrameTooLong> {
        let end = self.at.checked_add(bytes.len()).ok_or(FrameTooLong)?;
        self.buffer
            .get_mut(self.at..end)
            .ok_or(FrameTooLong)?
            .copy_from_slice(bytes);
        self.at = end;
        Ok(())
    }

    fn fill(&mut self, count: usize, value: u8) -> Result<(), FrameTooLong> {
        let end = self.at.checked_add(count).ok_or(FrameTooLong)?;
        self.buffer
            .get_mut(self.at..end)
            .ok_or(FrameTooLong)?
            .fill(value);
        self.at = end;
        Ok(())
    }

    /// A MAC header (9.3.3.2): frame control, duration, three addresses and
    /// the sequence number `n` modulo 4096.
    fn header(
        &mut self,
        frame_control: [u8; 2],
        duration: u16,
        addresses: [[u8; 6]; 3],
        n: u16,
    ) -> Result<(), FrameTooLong> {
        self.bytes(&frame_control)?;
        self.bytes(&duration.to_le_bytes())?;
        for address in addresses {
            self.bytes(&address)?;
        }
        self.bytes(&(n.wrapping_shl(4)).to_le_bytes())
    }

    fn tag(&mut self, kind: u8, arm: u8, n: u16, run: u32, k: u64) -> Result<(), FrameTooLong> {
        self.bytes(&TAG_HEAD)?;
        self.bytes(&[TAG_VERSION, kind, arm])?;
        self.bytes(&n.to_le_bytes())?;
        self.bytes(&run.to_le_bytes())?;
        self.bytes(&k.to_le_bytes())
    }

    /// SSID, Supported Rates (1, 2, 5.5 and 11 Mb/s, basic) and DS Parameter
    /// Set elements (9.4.2.2, 9.4.2.3, 9.4.2.4).
    fn bss_elements(&mut self, channel: u8) -> Result<(), FrameTooLong> {
        let length = u8::try_from(SSID.len()).map_err(|_| FrameTooLong)?;
        self.bytes(&[0, length])?;
        self.bytes(SSID)?;
        self.bytes(&[1, 4, 0x82, 0x84, 0x8b, 0x96])?;
        self.bytes(&[3, 1, channel])
    }
}

/// The addresses of a run: the AP's own and the absent receiver of the
/// individually addressed subject frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Addresses {
    pub(crate) ap: [u8; 6],
    pub(crate) absent: [u8; 6],
}

/// A locally administered individual address no station of the stand has.
pub(crate) const ABSENT: [u8; 6] = [0x02, 0x4f, 0x45, 0x00, 0x00, 0x01];
const BROADCAST: [u8; 6] = [0xff; 6];

/// One round's identity in its frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Round {
    pub(crate) n: u16,
    pub(crate) kind: Kind,
    pub(crate) arm: Arm,
    pub(crate) run: u32,
    pub(crate) k: u64,
    pub(crate) channel: u8,
}

/// Write round's subject MPDU into `buffer`; its length.
pub(crate) fn subject(
    buffer: &mut [u8],
    round: Round,
    addresses: Addresses,
) -> Result<usize, FrameTooLong> {
    let mut writer = Writer::new(buffer);
    let Round {
        n,
        kind,
        arm,
        run,
        k,
        channel,
    } = round;
    match kind {
        Kind::Beacon => {
            writer.header([0x80, 0], 0, [BROADCAST, addresses.ap, addresses.ap], n)?;
            writer.bytes(&k.to_le_bytes())?;
            writer.bytes(&BEACON_INTERVAL_TU.to_le_bytes())?;
            writer.bytes(&CAPABILITY.to_le_bytes())?;
            writer.bss_elements(channel)?;
            // TIM (9.4.2.5): DTIM count 0, period 1, no buffered traffic.
            writer.bytes(&[5, 4, 0, 1, 0, 0])?;
        }
        Kind::ProbeResponse => {
            writer.header(
                [0x50, 0],
                UNICAST_DURATION_US,
                [addresses.absent, addresses.ap, addresses.ap],
                n,
            )?;
            writer.bytes(&k.to_le_bytes())?;
            writer.bytes(&BEACON_INTERVAL_TU.to_le_bytes())?;
            writer.bytes(&CAPABILITY.to_le_bytes())?;
            writer.bss_elements(channel)?;
        }
        Kind::Action => {
            writer.header(
                [0xd0, 0],
                UNICAST_DURATION_US,
                [addresses.absent, addresses.ap, addresses.ap],
                n,
            )?;
            writer.bytes(&k.to_le_bytes())?;
        }
    }
    writer.tag(kind.code(), arm.code(), n, run, k)?;
    Ok(writer.at)
}

/// Write round `n`'s blocker MPDU into `buffer`; its length. A data frame
/// from the DS (FromDS, 9.2.4.1.4), group addressed: nothing answers it.
pub(crate) fn blocker(
    buffer: &mut [u8],
    n: u16,
    run: u32,
    ap: [u8; 6],
) -> Result<usize, FrameTooLong> {
    let mut writer = Writer::new(buffer);
    writer.header([0x08, 0x02], 0, [BROADCAST, ap, ap], n)?;
    writer.tag(BLOCKER_CODE, Arm::Delayed.code(), n, run, 0)?;
    writer.fill(BLOCKER_BODY.saturating_sub(TAG_LENGTH), 0xa5)?;
    Ok(writer.at)
}

/// The MPDU's first octet in the transmit buffer: the buffer starts with
/// the backend's metadata (`TX_METADATA_SIZE`).
pub(crate) const MPDU_OFFSET: usize = 8;
/// The FCS the hardware appends.
pub(crate) const FCS: usize = 4;
/// Octets 24..32 of the MPDU in the transmit buffer.
pub(crate) const STAMP_OFFSET: usize = MPDU_OFFSET + HEADER;

/// The descriptor's lengths of an MPDU of `mpdu` octets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Transfer {
    /// Metadata, MPDU and FCS.
    pub(crate) length: u32,
    /// The length rounded up to a word.
    pub(crate) capacity: u32,
    /// MPDU and FCS: the length the hardware sends.
    pub(crate) psdu: u16,
}

/// Write the metadata before an MPDU of `mpdu` octets and zero its FCS
/// space, as the backend's ordinary transmitter lays out its buffer: the
/// hardware frame length (MPDU and FCS) little-endian, then four zero
/// octets.
pub(crate) fn metadata(buffer: &mut [u8], mpdu: usize) -> Result<Transfer, FrameTooLong> {
    let psdu = mpdu.checked_add(FCS).ok_or(FrameTooLong)?;
    let length = MPDU_OFFSET.checked_add(psdu).ok_or(FrameTooLong)?;
    let capacity = length.checked_add(3).ok_or(FrameTooLong)? & !3;
    if capacity > buffer.len() {
        return Err(FrameTooLong);
    }
    let psdu32 = u32::try_from(psdu).map_err(|_| FrameTooLong)?;
    let (head, _) = buffer
        .split_at_mut_checked(MPDU_OFFSET)
        .ok_or(FrameTooLong)?;
    let (word, zero) = head.split_at_mut_checked(4).ok_or(FrameTooLong)?;
    word.copy_from_slice(&psdu32.to_le_bytes());
    zero.fill(0);
    let mpdu_end = MPDU_OFFSET.checked_add(mpdu).ok_or(FrameTooLong)?;
    buffer
        .get_mut(mpdu_end..capacity)
        .ok_or(FrameTooLong)?
        .fill(0);
    Ok(Transfer {
        length: u32::try_from(length).map_err(|_| FrameTooLong)?,
        capacity: u32::try_from(capacity).map_err(|_| FrameTooLong)?,
        psdu: u16::try_from(psdu).map_err(|_| FrameTooLong)?,
    })
}

/// Octets 24..32 of the MPDU in a transmit buffer.
pub(crate) fn stamp_octets(buffer: &[u8]) -> Option<[u8; 8]> {
    let end = STAMP_OFFSET.checked_add(8)?;
    buffer.get(STAMP_OFFSET..end)?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDRESSES: Addresses = Addresses {
        ap: [0x60, 0x55, 0xf9, 0x01, 0x02, 0x03],
        absent: ABSENT,
    };

    fn round_of(n: u16) -> Round {
        let (kind, arm) = round(n).unwrap();
        Round {
            n,
            kind,
            arm,
            run: 0x1234_5678,
            k: k_of(n),
            channel: 13,
        }
    }

    #[test]
    fn every_kind_runs_in_one_block_with_alternating_arms() {
        assert_eq!(round(0), Some((Kind::Beacon, Arm::Undelayed)));
        assert_eq!(round(1), Some((Kind::Beacon, Arm::Delayed)));
        assert_eq!(round(31), Some((Kind::Beacon, Arm::Delayed)));
        assert_eq!(round(32), Some((Kind::ProbeResponse, Arm::Undelayed)));
        assert_eq!(round(64), Some((Kind::Action, Arm::Undelayed)));
        assert_eq!(round(95), Some((Kind::Action, Arm::Delayed)));
        assert_eq!(round(ROUNDS), None);
    }

    #[test]
    fn k_changes_every_octet_of_both_words_and_stays_far_from_any_counter() {
        let mut seen = std::collections::HashSet::new();
        for n in 0..ROUNDS {
            let k = k_of(n);
            assert!(seen.insert(k), "K repeats at {n}");
            assert!(k >> 32 != 0 && k > 1 << 60);
            for octet in k.to_le_bytes() {
                assert!((0x11..=0xd0).contains(&octet), "{k:016x}");
            }
            if n > 0 {
                let previous = k_of(n - 1).to_le_bytes();
                for (a, b) in previous.iter().zip(k.to_le_bytes()) {
                    assert_ne!(*a, b, "an octet of K stays from round {} to {n}", n - 1);
                }
            }
        }
    }

    #[test]
    fn a_beacon_carries_k_as_its_timestamp_and_the_tag_after_its_elements() {
        let mut buffer = [0u8; 256];
        let round = round_of(4);
        let length = subject(&mut buffer, round, ADDRESSES).unwrap();
        let frame = &buffer[..length];
        assert_eq!(frame[..2], [0x80, 0]);
        assert_eq!(frame[4..10], [0xff; 6]);
        assert_eq!(frame[10..16], ADDRESSES.ap);
        assert_eq!(u16::from_le_bytes([frame[22], frame[23]]), 4 << 4);
        assert_eq!(frame[24..32], k_of(4).to_le_bytes());
        assert_eq!(u16::from_le_bytes([frame[32], frame[33]]), 100);
        let tag = &frame[length - TAG_LENGTH..];
        assert_eq!(tag[..9], TAG_HEAD);
        assert_eq!(tag[9..12], [1, b'b', b'u']);
        assert_eq!(u16::from_le_bytes([tag[12], tag[13]]), 4);
        assert_eq!(tag[18..26], k_of(4).to_le_bytes());
    }

    #[test]
    fn probe_responses_and_actions_go_to_the_absent_receiver_with_k_at_24() {
        let mut buffer = [0u8; 256];
        for (n, code) in [(33, 0x50), (65, 0xd0)] {
            let length = subject(&mut buffer, round_of(n), ADDRESSES).unwrap();
            let frame = &buffer[..length];
            assert_eq!(frame[0], code);
            assert_eq!(frame[4..10], ABSENT);
            assert_eq!(frame[24..32], k_of(n).to_le_bytes());
            assert_eq!(frame[length - TAG_LENGTH + 11], b'd');
        }
        let length = subject(&mut buffer, round_of(64), ADDRESSES).unwrap();
        assert_eq!(length, 24 + 8 + TAG_LENGTH);
    }

    #[test]
    fn the_blocker_is_a_group_data_frame_of_1500_body_octets() {
        let mut buffer = [0u8; 1536];
        let length = blocker(&mut buffer[MPDU_OFFSET..], 7, 1, ADDRESSES.ap).unwrap();
        assert_eq!(length, 24 + BLOCKER_BODY);
        let frame = &buffer[MPDU_OFFSET..MPDU_OFFSET + length];
        assert_eq!(frame[..2], [0x08, 0x02]);
        assert_eq!(frame[4..10], [0xff; 6]);
        assert_eq!(frame[24..33], TAG_HEAD);
        assert_eq!(frame[34], BLOCKER_CODE);
        let transfer = metadata(&mut buffer, length).unwrap();
        assert_eq!(transfer.psdu, 1528);
        assert_eq!(transfer.length, 1536);
        assert_eq!(transfer.capacity, 1536);
        assert_eq!(buffer[..8], [0xf8, 0x05, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn the_metadata_zeroes_the_fcs_space_and_refuses_a_short_buffer() {
        let mut buffer = [0xeeu8; 128];
        let length = subject(&mut buffer[MPDU_OFFSET..], round_of(64), ADDRESSES).unwrap();
        let transfer = metadata(&mut buffer, length).unwrap();
        assert_eq!(transfer.length as usize, MPDU_OFFSET + length + FCS);
        assert_eq!(transfer.capacity % 4, 0);
        let end = MPDU_OFFSET + length;
        assert!(
            buffer[end..transfer.capacity as usize]
                .iter()
                .all(|b| *b == 0)
        );
        assert_eq!(stamp_octets(&buffer), Some(k_of(64).to_le_bytes()));
        assert_eq!(metadata(&mut [0u8; 16], 24), Err(FrameTooLong));
        assert_eq!(
            subject(&mut [0u8; 40], round_of(0), ADDRESSES),
            Err(FrameTooLong)
        );
    }
}
