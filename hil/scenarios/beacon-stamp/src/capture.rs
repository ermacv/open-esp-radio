//! A monitor capture: a classic pcap file of 802.11 frames with radiotap
//! headers (link type 127), as `tcpdump -w` writes it on a monitor
//! interface. Each frame keeps the sniffer's TSFT (radiotap field 0: the
//! sniffer MAC's TSF in microseconds), its transmitter address, its octets
//! 24..32 and the experiment's tag when it carries one. pcapng is not read:
//! `editcap -F pcap` converts it.

/// The experiment's tag element (the probe's `frames.rs`): identifier 0xdd,
/// length 24, OUI 02:4f:45, "STMP".
const TAG_HEAD: [u8; 9] = [0xdd, 24, 0x02, 0x4f, 0x45, b'S', b'T', b'M', b'P'];
const TAG_LENGTH: usize = 26;
const LINKTYPE_IEEE802_11_RADIOTAP: u32 = 127;
/// Radiotap Flags (field 1): the frame ends with its FCS; the FCS failed.
const FLAG_FCS_AT_END: u8 = 0x10;
const FLAG_BAD_FCS: u8 = 0x40;

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
    pub transmitter: [u8; 6],
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

/// The radiotap header's length, its TSFT and its Flags. Fields 0 (TSFT,
/// eight octets aligned to eight) and 1 (Flags, one octet) are the first
/// of the first presence word, so their place follows from the number of
/// presence words alone (radiotap.org, "Alignment in Radiotap").
fn radiotap(packet: &[u8]) -> Option<(usize, Option<u64>, u8)> {
    if *packet.first()? != 0 {
        return None;
    }
    let length = usize::from(u16_at(packet, 2, false)?);
    let present = u32_at(packet, 4, false)?;
    let mut words = 1usize;
    let mut last = present;
    while last & 0x8000_0000 != 0 {
        last = u32_at(packet, 4usize.checked_add(words.checked_mul(4)?)?, false)?;
        words = words.checked_add(1)?;
    }
    let mut at = 4usize.checked_add(words.checked_mul(4)?)?;
    let mut tsft = None;
    if present & 1 != 0 {
        at = at.checked_add(7)? & !7;
        tsft = Some(u64_le(packet, at)?);
        at = at.checked_add(8)?;
    }
    let mut flags = 0;
    if present & 2 != 0 {
        flags = *packet.get(at)?;
    }
    if length > packet.len() {
        return None;
    }
    Some((length, tsft, flags))
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
fn frame(tsft: u64, mpdu: &[u8]) -> Option<AirFrame> {
    let frame_control = *mpdu.first()?;
    let transmitter: [u8; 6] = mpdu.get(10..16)?.try_into().ok()?;
    let stamp = mpdu.get(24..32).and_then(|octets| octets.try_into().ok());
    // A data frame's tag starts its body; a management frame's follows K.
    let from = if frame_control & 0x0c == 0x08 { 24 } else { 32 };
    Some(AirFrame {
        tsft,
        frame_control,
        transmitter,
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
        let Some((length, tsft, flags)) = radiotap(packet) else {
            return Err(format!("record {index} has no valid radiotap header"));
        };
        if flags & FLAG_BAD_FCS != 0 {
            capture.bad_fcs = capture.bad_fcs.saturating_add(1);
            continue;
        }
        let Some(tsft) = tsft else {
            capture.without_tsft = capture.without_tsft.saturating_add(1);
            continue;
        };
        let mut mpdu = packet.get(length..).unwrap_or_default();
        if flags & FLAG_FCS_AT_END != 0 {
            mpdu = mpdu.get(..mpdu.len().saturating_sub(4)).unwrap_or_default();
        }
        if let Some(frame) = frame(tsft, mpdu) {
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
        let mut packet = vec![0, 0, 0, 0];
        packet.extend_from_slice(&0x0000_0003u32.to_le_bytes());
        packet.extend_from_slice(&tsft.to_le_bytes());
        packet.push(0x10);
        packet.extend_from_slice(&[0; 3]);
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
        assert_eq!(frame.transmitter, TA);
        assert_eq!(frame.stamp, Some(0x1122_3344_5566_7788u64.to_le_bytes()));
        assert_eq!(frame.tag, Some(TAG));
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
