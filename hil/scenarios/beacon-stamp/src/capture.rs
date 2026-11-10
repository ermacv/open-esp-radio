//! A monitor capture: a classic pcap file of 802.11 frames with radiotap
//! headers (link type 127), as `tcpdump -w` writes it on a monitor
//! interface. Each frame keeps the sniffer's TSFT (radiotap field 0: the
//! sniffer MAC's TSF in microseconds), its transmitter address, its octets
//! 24..32 and the experiment's tag when it carries one. pcapng is not read:
//! `editcap -F pcap` converts it.

use crate::airtime::Phy;

/// The experiment's tag element (the probe's `frames.rs`): identifier 0xdd,
/// length 24, OUI 02:4f:45, "STMP".
const TAG_HEAD: [u8; 9] = [0xdd, 24, 0x02, 0x4f, 0x45, b'S', b'T', b'M', b'P'];
const TAG_LENGTH: usize = 26;
const LINKTYPE_IEEE802_11_RADIOTAP: u32 = 127;
/// Radiotap Flags (field 1): the frame ends with its FCS; the FCS failed.
const FLAG_FCS_AT_END: u8 = 0x10;
const FLAG_BAD_FCS: u8 = 0x40;

/// The PHY a frame's radiotap header reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservedPhy {
    /// No Rate or MCS field, or an MCS field without its bandwidth, index
    /// or guard interval.
    Missing,
    /// A PHY the calibration does not use.
    Unsupported,
    Phy(Phy),
}

/// What a frame's tag says of it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tag {
    pub kind: u8,
    pub arm: u8,
    pub n: u16,
    pub run: u32,
    pub k: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AirFrame {
    /// The sniffer's TSFT, microseconds.
    pub tsft: u64,
    /// Frame Control's first octet: type and subtype.
    pub frame_control: u8,
    /// Address 1.
    pub receiver: [u8; 6],
    /// Address 2; a control frame such as an ACK has none.
    pub transmitter: Option<[u8; 6]>,
    /// The PSDU: the MPDU and its FCS, octets.
    pub psdu: u32,
    /// The PHY its radiotap header reports.
    pub phy: ObservedPhy,
    /// MPDU octets 24..32.
    pub stamp: Option<[u8; 8]>,
    pub tag: Option<Tag>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Capture {
    pub frames: Vec<AirFrame>,
    /// Records without a radiotap TSFT.
    pub without_tsft: usize,
    /// Records whose radiotap flags report a failed FCS.
    pub bad_fcs: usize,
}

fn u16_at(bytes: &[u8], at: usize, big: bool) -> Option<u16> {
    let raw: [u8; 2] = bytes.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(if big {
        u16::from_be_bytes(raw)
    } else {
        u16::from_le_bytes(raw)
    })
}

fn u32_at(bytes: &[u8], at: usize, big: bool) -> Option<u32> {
    let raw: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(if big {
        u32::from_be_bytes(raw)
    } else {
        u32::from_le_bytes(raw)
    })
}

fn u64_le(bytes: &[u8], at: usize) -> Option<u64> {
    let raw: [u8; 8] = bytes.get(at..at.checked_add(8)?)?.try_into().ok()?;
    Some(u64::from_le_bytes(raw))
}

/// The radiotap fields 0..=19 of the first presence word: alignment and
/// size (radiotap.org, "Defined fields"). Their places follow from this
/// order alone: every field of the first word precedes any later word's.
const FIELDS: [(usize, usize); 20] = [
    (8, 8), // 0 TSFT
    (1, 1), // 1 Flags
    (1, 1), // 2 Rate, 500 kb/s
    (2, 4), // 3 Channel
    (2, 2), // 4 FHSS
    (1, 1), // 5 dBm antenna signal
    (1, 1), // 6 dBm antenna noise
    (2, 2), // 7 Lock quality
    (2, 2), // 8 TX attenuation
    (2, 2), // 9 dB TX attenuation
    (1, 1), // 10 dBm TX power
    (1, 1), // 11 Antenna
    (1, 1), // 12 dB antenna signal
    (1, 1), // 13 dB antenna noise
    (2, 2), // 14 RX flags
    (2, 2), // 15 TX flags
    (1, 1), // 16 RTS retries
    (1, 1), // 17 data retries
    (4, 8), // 18 XChannel
    (1, 3), // 19 MCS: known, flags, index
];

/// What a radiotap header says of its frame.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Radiotap {
    length: usize,
    tsft: Option<u64>,
    flags: u8,
    rate: Option<u8>,
    mcs: Option<[u8; 3]>,
}

/// The radiotap header of `packet`, every read bounded by its `it_len`.
/// A field the presence words do not declare is absent. The header is
/// refused, with its reason, for an unknown version, an `it_len` shorter
/// than the first presence word or longer than the packet, and a declared
/// presence word or field that does not fit within `it_len`: such a record
/// is malformed, never read with unknown fields or with octets of its frame.
fn radiotap(packet: &[u8]) -> Result<Radiotap, String> {
    if packet.first() != Some(&0) {
        return Err("radiotap version is not 0".into());
    }
    let length = usize::from(u16_at(packet, 2, false).ok_or("radiotap it_len is cut")?);
    if length < 8 || length > packet.len() {
        return Err(format!(
            "radiotap it_len {length} is outside 8..={}",
            packet.len()
        ));
    }
    let bounded = packet.get(..length).ok_or("radiotap it_len is cut")?;
    let mut header = Radiotap {
        length,
        ..Radiotap::default()
    };
    let present = u32_at(bounded, 4, false).ok_or("radiotap presence word is cut")?;
    let mut words = 1usize;
    let mut last = present;
    while last & 0x8000_0000 != 0 {
        let at = words
            .checked_mul(4)
            .and_then(|offset| offset.checked_add(4))
            .ok_or("radiotap presence words overflow")?;
        last = u32_at(bounded, at, false).ok_or_else(|| {
            format!("radiotap presence word {words} at {at} lies past it_len {length}")
        })?;
        words = words
            .checked_add(1)
            .ok_or("radiotap presence words overflow")?;
    }
    let mut at = words
        .checked_mul(4)
        .and_then(|size| size.checked_add(4))
        .ok_or("radiotap presence words overflow")?;
    for (bit, (align, size)) in FIELDS.iter().enumerate() {
        if present & (1 << bit) == 0 {
            continue;
        }
        let start = at
            .checked_add(align - 1)
            .map(|padded| padded / align * align)
            .ok_or("radiotap field offset overflows")?;
        let end = start
            .checked_add(*size)
            .ok_or("radiotap field offset overflows")?;
        let field = bounded.get(start..end).ok_or_else(|| {
            format!("radiotap field {bit} at {start}..{end} lies past it_len {length}")
        })?;
        match bit {
            0 => header.tsft = u64_le(field, 0),
            1 => header.flags = field.first().copied().unwrap_or(0),
            2 => header.rate = field.first().copied(),
            19 => header.mcs = field.try_into().ok(),
            _ => {}
        }
        at = end;
    }
    Ok(header)
}

/// The PHY a radiotap header describes, as mac80211 v6.18.26 writes it
/// (`net/mac80211/rx.c`, `ieee80211_add_rx_radiotap_header`): the Rate
/// field for a non-HT frame, the MCS field for HT. mac80211 marks the
/// format and the FEC known only when they are greenfield or LDPC
/// (rx.c 485-489): their absence means HT-mixed and BCC there. Anything
/// else is not a PHY the calibration uses.
fn observed_phy(header: &Radiotap) -> ObservedPhy {
    const SHORT_PREAMBLE: u8 = 0x02;
    const HAVE_BW: u8 = 0x01;
    const HAVE_MCS: u8 = 0x02;
    const HAVE_GI: u8 = 0x04;
    const HAVE_FMT: u8 = 0x08;
    const HAVE_FEC: u8 = 0x10;
    match (header.mcs, header.rate) {
        (Some([known, flags, index]), _) => {
            let required = HAVE_BW | HAVE_MCS | HAVE_GI;
            if known & required != required {
                return ObservedPhy::Missing;
            }
            let greenfield = known & HAVE_FMT != 0 && flags & 0x08 != 0;
            let ldpc = known & HAVE_FEC != 0 && flags & 0x10 != 0;
            // 20 MHz, long GI, no STBC, mixed, BCC, MCS 0..=7.
            if flags & 0x03 != 0
                || flags & 0x04 != 0
                || flags & 0x60 != 0
                || greenfield
                || ldpc
                || index > 7
            {
                return ObservedPhy::Unsupported;
            }
            ObservedPhy::Phy(Phy::HtMixed { mcs: index })
        }
        (None, Some(rate)) => match rate {
            2 | 4 | 11 | 22 => ObservedPhy::Phy(Phy::Dsss {
                rate_kbps: u32::from(rate) * 500,
                short: header.flags & SHORT_PREAMBLE != 0,
            }),
            12 | 18 | 24 | 36 | 48 | 72 | 96 | 108 => ObservedPhy::Phy(Phy::ErpOfdm {
                rate_mbps: u32::from(rate) / 2,
            }),
            _ => ObservedPhy::Unsupported,
        },
        (None, None) => ObservedPhy::Missing,
    }
}

fn tag(mpdu: &[u8], from: usize) -> Option<Tag> {
    let body = mpdu.get(from..)?;
    let start = body
        .windows(TAG_HEAD.len())
        .position(|window| window == TAG_HEAD)?;
    let tag = body.get(start..start.checked_add(TAG_LENGTH)?)?;
    if *tag.get(9)? != 1 {
        return None;
    }
    Some(Tag {
        kind: *tag.get(10)?,
        arm: *tag.get(11)?,
        n: u16_at(tag, 12, false)?,
        run: u32_at(tag, 14, false)?,
        k: u64_le(tag, 18)?,
    })
}

/// The frame of one radiotap record; `None` for a record too short to be
/// an 802.11 header.
fn frame(tsft: u64, phy: ObservedPhy, mpdu: &[u8]) -> Option<AirFrame> {
    let frame_control = *mpdu.first()?;
    let receiver: [u8; 6] = mpdu.get(4..10)?.try_into().ok()?;
    let transmitter = mpdu.get(10..16).and_then(|octets| octets.try_into().ok());
    let psdu = u32::try_from(mpdu.len()).ok()?.checked_add(4)?;
    let stamp = mpdu.get(24..32).and_then(|octets| octets.try_into().ok());
    // A data frame's tag starts its body; a management frame's follows K.
    let from = if frame_control & 0x0c == 0x08 { 24 } else { 32 };
    Some(AirFrame {
        tsft,
        frame_control,
        receiver,
        transmitter,
        psdu,
        phy,
        stamp,
        tag: tag(mpdu, from),
    })
}

/// A whole capture file: a truncated last record is an error.
pub fn parse(bytes: &[u8]) -> Result<Capture, String> {
    parse_with(bytes, false)
}

/// The complete records of a capture still being written: its last
/// record may be incomplete and is left out.
pub fn parse_prefix(bytes: &[u8]) -> Result<Capture, String> {
    parse_with(bytes, true)
}

fn parse_with(bytes: &[u8], partial_tail: bool) -> Result<Capture, String> {
    let magic = u32_at(bytes, 0, false).ok_or("the capture has no pcap header")?;
    let big = match magic {
        0xa1b2_c3d4 | 0xa1b2_3c4d => false,
        0xd4c3_b2a1 | 0x4d3c_b2a1 => true,
        0x0a0d_0d0a => return Err("a pcapng capture: convert it with `editcap -F pcap`".into()),
        other => return Err(format!("not a pcap capture (magic {other:08x})")),
    };
    let link = u32_at(bytes, 20, big).ok_or("the pcap header is short")?;
    if link != LINKTYPE_IEEE802_11_RADIOTAP {
        return Err(format!(
            "link type {link}, not 802.11 with radiotap (127): capture on a monitor interface"
        ));
    }
    let mut capture = Capture::default();
    let mut at = 24usize;
    let mut index = 0usize;
    while at < bytes.len() {
        let Some(included) = u32_at(bytes, at.checked_add(8).ok_or("offset")?, big) else {
            if partial_tail {
                break;
            }
            return Err(format!("record {index} has a short header"));
        };
        let start = at.checked_add(16).ok_or("offset")?;
        let end = start
            .checked_add(usize::try_from(included).map_err(|_| "record length")?)
            .ok_or("offset")?;
        let Some(packet) = bytes.get(start..end) else {
            if partial_tail {
                break;
            }
            return Err(format!("record {index} runs past the end of the capture"));
        };
        at = end;
        index = index.saturating_add(1);
        let header = radiotap(packet).map_err(|reason| format!("record {index}: {reason}"))?;
        let (length, flags) = (header.length, header.flags);
        if flags & FLAG_BAD_FCS != 0 {
            capture.bad_fcs = capture.bad_fcs.saturating_add(1);
            continue;
        }
        let Some(tsft) = header.tsft else {
            capture.without_tsft = capture.without_tsft.saturating_add(1);
            continue;
        };
        let mut mpdu = packet.get(length..).unwrap_or_default();
        if flags & FLAG_FCS_AT_END != 0 {
            mpdu = mpdu.get(..mpdu.len().saturating_sub(4)).unwrap_or_default();
        }
        if let Some(frame) = frame(tsft, observed_phy(&header), mpdu) {
            capture.frames.push(frame);
        }
    }
    Ok(capture)
}

/// Test captures: the bytes `tcpdump -w` would write for these frames.
#[cfg(test)]
pub mod synthetic {
    /// One radiotap record: TSFT and Flags (FCS at end), the MPDU and a
    /// dummy FCS.
    pub fn record(tsft: u64, mpdu: &[u8]) -> Vec<u8> {
        record_with(tsft, None, None, 0, mpdu)
    }

    /// A radiotap record with an optional Rate field (500 kb/s), an
    /// optional MCS field (known, flags, index) and extra Flags bits.
    pub fn record_with(
        tsft: u64,
        rate: Option<u8>,
        mcs: Option<[u8; 3]>,
        flags: u8,
        mpdu: &[u8],
    ) -> Vec<u8> {
        let present = 0x3u32
            | if rate.is_some() { 1 << 2 } else { 0 }
            | if mcs.is_some() { 1 << 19 } else { 0 };
        let mut packet = vec![0, 0, 0, 0];
        packet.extend_from_slice(&present.to_le_bytes());
        packet.extend_from_slice(&tsft.to_le_bytes());
        packet.push(0x10 | flags);
        if let Some(rate) = rate {
            packet.push(rate);
        }
        if let Some(mcs) = mcs {
            packet.extend_from_slice(&mcs);
        }
        while packet.len() % 4 != 0 {
            packet.push(0);
        }
        let length = packet.len() as u16;
        packet[2..4].copy_from_slice(&length.to_le_bytes());
        packet.extend_from_slice(mpdu);
        packet.extend_from_slice(&[0xee; 4]);
        packet
    }

    pub fn file(records: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&[0; 8]);
        bytes.extend_from_slice(&65535u32.to_le_bytes());
        bytes.extend_from_slice(&127u32.to_le_bytes());
        for record in records {
            bytes.extend_from_slice(&[0; 8]);
            bytes.extend_from_slice(&(record.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&(record.len() as u32).to_le_bytes());
            bytes.extend_from_slice(record);
        }
        bytes
    }

    /// A tagged MPDU: `frame_control`, transmitter `ta`, `stamp` at 24 for
    /// a management frame, then the tag.
    pub fn mpdu(frame_control: u8, ta: [u8; 6], stamp: u64, tag: super::Tag) -> Vec<u8> {
        let mut mpdu = vec![frame_control, 0, 0, 0];
        mpdu.extend_from_slice(&[0xff; 6]);
        mpdu.extend_from_slice(&ta);
        mpdu.extend_from_slice(&ta);
        mpdu.extend_from_slice(&(tag.n << 4).to_le_bytes());
        if frame_control & 0x0c != 0x08 {
            mpdu.extend_from_slice(&stamp.to_le_bytes());
            mpdu.extend_from_slice(&[0x64, 0, 1, 0]);
        }
        mpdu.extend_from_slice(&super::TAG_HEAD);
        mpdu.extend_from_slice(&[1, tag.kind, tag.arm]);
        mpdu.extend_from_slice(&tag.n.to_le_bytes());
        mpdu.extend_from_slice(&tag.run.to_le_bytes());
        mpdu.extend_from_slice(&tag.k.to_le_bytes());
        mpdu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TA: [u8; 6] = [0x60, 0x55, 0xf9, 1, 2, 3];
    const TAG: Tag = Tag {
        kind: b'b',
        arm: b'd',
        n: 9,
        run: 0xdead_beef,
        k: 0x5a5a_a5a5_0123_4567,
    };

    #[test]
    fn a_tagged_beacon_keeps_its_tsft_stamp_and_tag() {
        let mpdu = synthetic::mpdu(0x80, TA, 0x1122_3344_5566_7788, TAG);
        let bytes = synthetic::file(&[synthetic::record(123_456, &mpdu)]);
        let capture = parse(&bytes).unwrap();
        assert_eq!(capture.frames.len(), 1);
        let frame = &capture.frames[0];
        assert_eq!(frame.tsft, 123_456);
        assert_eq!(frame.frame_control, 0x80);
        assert_eq!(frame.transmitter, Some(TA));
        assert_eq!(frame.stamp, Some(0x1122_3344_5566_7788u64.to_le_bytes()));
        assert_eq!(frame.tag, Some(TAG));
    }

    #[test]
    fn the_radiotap_rate_and_mcs_give_the_observed_phy() {
        let mpdu = synthetic::mpdu(0x80, TA, 1, TAG);
        let phy = |record: Vec<u8>| parse(&synthetic::file(&[record])).unwrap().frames[0].phy;
        assert_eq!(phy(synthetic::record(1, &mpdu)), ObservedPhy::Missing);
        assert_eq!(
            phy(synthetic::record_with(1, Some(2), None, 0, &mpdu)),
            ObservedPhy::Phy(Phy::Dsss {
                rate_kbps: 1000,
                short: false
            })
        );
        assert_eq!(
            phy(synthetic::record_with(1, Some(22), None, 0x02, &mpdu)),
            ObservedPhy::Phy(Phy::Dsss {
                rate_kbps: 11000,
                short: true
            })
        );
        assert_eq!(
            phy(synthetic::record_with(1, Some(12), None, 0, &mpdu)),
            ObservedPhy::Phy(Phy::ErpOfdm { rate_mbps: 6 })
        );
        // mac80211's HT: bandwidth, index and GI known; mixed and BCC.
        assert_eq!(
            phy(synthetic::record_with(
                1,
                None,
                Some([0x07, 0, 0]),
                0,
                &mpdu
            )),
            ObservedPhy::Phy(Phy::HtMixed { mcs: 0 })
        );
        // Without the GI known, or at 40 MHz, short GI, greenfield, LDPC.
        assert_eq!(
            phy(synthetic::record_with(
                1,
                None,
                Some([0x03, 0, 0]),
                0,
                &mpdu
            )),
            ObservedPhy::Missing
        );
        for (known, flags) in [(0x07, 0x01), (0x07, 0x04), (0x0f, 0x08), (0x17, 0x10)] {
            assert_eq!(
                phy(synthetic::record_with(
                    1,
                    None,
                    Some([known, flags, 0]),
                    0,
                    &mpdu
                )),
                ObservedPhy::Unsupported,
                "{known:02x} {flags:02x}"
            );
        }
        assert_eq!(
            phy(synthetic::record_with(1, Some(5), None, 0, &mpdu)),
            ObservedPhy::Unsupported
        );
    }

    /// A record of a radiotap header of `it_len` octets whose words are
    /// `header` (padded to `it_len`), then the MPDU.
    fn record_of(header: &[u8], it_len: u16, mpdu: &[u8]) -> Vec<u8> {
        let mut packet = header.to_vec();
        packet.resize(usize::from(it_len), 0);
        packet[2..4].copy_from_slice(&it_len.to_le_bytes());
        packet.extend_from_slice(mpdu);
        packet
    }

    /// The record alone as a complete capture, and the same record followed
    /// by an incomplete last record, as a live capture.
    fn both_ways(record: Vec<u8>) -> (Result<Capture, String>, Result<Capture, String>) {
        let whole = synthetic::file(std::slice::from_ref(&record));
        let mpdu = synthetic::mpdu(0x80, TA, 2, TAG);
        let mut live = synthetic::file(&[record, synthetic::record(4, &mpdu)]);
        live.truncate(live.len() - 3);
        (parse(&whole), parse_prefix(&live))
    }

    #[test]
    fn the_owners_rate_declared_past_it_len_refuses_the_record() {
        // 17 octets: version, pad, length, presence (TSFT, Flags, Rate),
        // TSFT at 8..16, Flags at 16; the declared Rate would be at 17.
        let mut header = vec![0, 0, 0, 0];
        header.extend_from_slice(&0b111u32.to_le_bytes());
        header.extend_from_slice(&77u64.to_le_bytes());
        header.push(0);
        // The MPDU's first octet, 0x02 (1 Mb/s as a Rate), right after.
        let mut mpdu = synthetic::mpdu(0x80, TA, 1, TAG);
        mpdu[0] = 0x02;
        let (whole, live) = both_ways(record_of(&header, 17, &mpdu));
        let reason = whole.unwrap_err();
        assert!(
            reason.contains("record 1") && reason.contains("field 2"),
            "{reason}"
        );
        assert!(live.unwrap_err().contains("field 2"));
    }

    #[test]
    fn the_owners_presence_word_declared_past_it_len_refuses_the_record() {
        // A first word announcing a second one, with it_len 8: the second
        // word would lie in the frame body.
        let mut header = vec![0, 0, 0, 0];
        header.extend_from_slice(&(0x8000_0007u32).to_le_bytes());
        let mut mpdu = 0x0000_0001u32.to_le_bytes().to_vec();
        mpdu.extend_from_slice(&synthetic::mpdu(0x80, TA, 1, TAG));
        let (whole, live) = both_ways(record_of(&header, 8, &mpdu));
        let reason = whole.unwrap_err();
        assert!(reason.contains("presence word 1"), "{reason}");
        assert!(live.unwrap_err().contains("presence word 1"));
    }

    #[test]
    fn an_mcs_field_declared_past_it_len_refuses_the_record() {
        // TSFT 8..16, Flags 16, MCS 17..20, but it_len 19.
        let mut header = vec![0, 0, 0, 0];
        header.extend_from_slice(&(0x3u32 | 1 << 19).to_le_bytes());
        header.extend_from_slice(&5u64.to_le_bytes());
        header.extend_from_slice(&[0, 0x07, 0x00]);
        let mpdu = synthetic::mpdu(0x80, TA, 1, TAG);
        let (whole, live) = both_ways(record_of(&header, 19, &mpdu));
        assert!(whole.unwrap_err().contains("field 19"));
        assert!(live.unwrap_err().contains("field 19"));
        // Whole, the same field is read.
        let mut complete = header.clone();
        complete.push(0);
        let capture = parse(&synthetic::file(&[record_of(&complete, 20, &mpdu)])).unwrap();
        assert_eq!(
            capture.frames[0].phy,
            ObservedPhy::Phy(Phy::HtMixed { mcs: 0 })
        );
    }

    #[test]
    fn a_tsft_declared_past_it_len_refuses_the_record() {
        let mut header = vec![0, 0, 0, 0];
        header.extend_from_slice(&1u32.to_le_bytes());
        let mut mpdu = 9u64.to_le_bytes().to_vec();
        mpdu.extend_from_slice(&synthetic::mpdu(0x80, TA, 1, TAG));
        let (whole, live) = both_ways(record_of(&header, 8, &mpdu));
        assert!(whole.unwrap_err().contains("field 0"));
        assert!(live.unwrap_err().contains("field 0"));
    }

    #[test]
    fn the_routers_own_mldv2_report_in_a_radiotap_capture_refuses_it() {
        // Lease #1317's first record, as captured: an Ethernet-framed MLDv2
        // report (ff02::16, ICMPv6 143) the router's kernel sent on its
        // monitor, under link type 127. It is no radiotap header, so the
        // capture is refused, complete or live, never read around it.
        const RECORD: &str = "ffffffffffffbefce7aebbd386dd600000000024000100000000000000000000000000000000ff0200000000000000000000000000163a000502000001008f006e8a0000000104000000ff020000000000000000000000000002";
        let packet: Vec<u8> = (0..RECORD.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&RECORD[at..at + 2], 16).unwrap())
            .collect();
        assert_eq!(packet.len(), 90);
        let mpdu = synthetic::mpdu(0x80, TA, 1, TAG);
        let (whole, live) = both_ways(packet.clone());
        assert!(
            whole
                .unwrap_err()
                .contains("record 1: radiotap version is not 0")
        );
        assert!(live.unwrap_err().contains("radiotap version is not 0"));
        // Behind valid frames too.
        let file = synthetic::file(&[synthetic::record(3, &mpdu), packet]);
        assert!(
            parse(&file)
                .unwrap_err()
                .contains("record 2: radiotap version is not 0")
        );
    }

    #[test]
    fn a_field_the_presence_words_do_not_declare_is_absent() {
        // TSFT and Flags only: no Rate and no MCS, accepted with no PHY.
        let mpdu = synthetic::mpdu(0x80, TA, 1, TAG);
        let capture = parse(&synthetic::file(&[synthetic::record(3, &mpdu)])).unwrap();
        assert_eq!(capture.frames[0].phy, ObservedPhy::Missing);
        assert_eq!(capture.frames[0].tsft, 3);
    }

    #[test]
    fn a_live_capture_leaves_out_only_its_incomplete_last_record() {
        let mpdu = synthetic::mpdu(0x80, TA, 1, TAG);
        let (whole, live) = both_ways(synthetic::record(3, &mpdu));
        assert_eq!(whole.unwrap().frames.len(), 1);
        let live = live.unwrap();
        assert_eq!(live.frames.len(), 1);
        assert_eq!(live.frames[0].tsft, 3);
    }

    #[test]
    fn a_data_frames_tag_starts_its_body() {
        let tag = Tag {
            kind: b'x',
            k: 0,
            ..TAG
        };
        let mpdu = synthetic::mpdu(0x08, TA, 0, tag);
        let capture = parse(&synthetic::file(&[synthetic::record(7, &mpdu)])).unwrap();
        assert_eq!(capture.frames[0].tag, Some(tag));
    }

    #[test]
    fn records_without_tsft_or_with_a_bad_fcs_are_counted_not_kept() {
        let mpdu = synthetic::mpdu(0x80, TA, 1, TAG);
        let mut bad = synthetic::record(1, &mpdu);
        bad[16] = 0x50;
        let mut untimed = vec![0, 0, 8, 0, 0, 0, 0, 0];
        untimed.extend_from_slice(&mpdu);
        let capture = parse(&synthetic::file(&[bad, untimed])).unwrap();
        assert!(capture.frames.is_empty());
        assert_eq!((capture.bad_fcs, capture.without_tsft), (1, 1));
    }

    #[test]
    fn an_extended_presence_bitmap_moves_the_tsft_to_its_alignment() {
        // Two presence words: the TSFT starts at 16, eight-aligned.
        let mut packet = vec![0, 0, 0, 0];
        packet.extend_from_slice(&0x8000_0001u32.to_le_bytes());
        packet.extend_from_slice(&0u32.to_le_bytes());
        packet.extend_from_slice(&[0; 4]);
        packet.extend_from_slice(&99u64.to_le_bytes());
        let length = packet.len() as u16;
        packet[2..4].copy_from_slice(&length.to_le_bytes());
        packet.extend_from_slice(&synthetic::mpdu(0x80, TA, 1, TAG));
        let capture = parse(&synthetic::file(&[packet])).unwrap();
        assert_eq!(capture.frames[0].tsft, 99);
        assert_eq!(capture.frames[0].tag, Some(TAG));
    }

    #[test]
    fn a_capture_still_being_written_keeps_its_complete_records() {
        let mpdu = synthetic::mpdu(0x80, TA, 1, TAG);
        let mut bytes =
            synthetic::file(&[synthetic::record(5, &mpdu), synthetic::record(6, &mpdu)]);
        bytes.truncate(bytes.len() - 7);
        assert!(parse(&bytes).is_err());
        assert_eq!(parse_prefix(&bytes).unwrap().frames.len(), 1);
        assert_eq!(parse_prefix(&bytes[..30]).unwrap().frames.len(), 0);
        assert!(
            parse_prefix(&bytes[..10]).is_err(),
            "no header yet is no capture"
        );
    }

    #[test]
    fn other_files_are_refused_with_a_reason() {
        assert!(parse(&[]).is_err());
        assert!(
            parse(&0x0a0d_0d0au32.to_le_bytes())
                .unwrap_err()
                .contains("pcapng")
        );
        let mut ethernet = synthetic::file(&[]);
        ethernet[20] = 1;
        assert!(parse(&ethernet).unwrap_err().contains("link type 1"));
        let mut truncated = synthetic::file(&[synthetic::record(1, &[0x80; 40])]);
        truncated.truncate(truncated.len() - 3);
        assert!(parse(&truncated).is_err());
    }
}
