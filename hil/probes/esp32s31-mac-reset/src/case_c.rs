//! Case C's frames (`tasks/154`, with `159`'s provenance series): one layout
//! for the sender (`esp32s31-mac-reset`) and the receiver
//! (`esp32s31-rx-ring`, which includes this file).
//!
//! Every frame carries its counter, the case in the top four bits and the
//! round below:
//!
//! 1. QoS Data, A-MSDU Present, two short subframes;
//! 2. the same with one subframe;
//! 3. the control: case 1's bytes with A-MSDU Present clear;
//! 4. QoS Data, A-MSDU Present, two long subframes: longer than a buffer;
//! 5. and 6. non-QoS Data of 700 and 1 500 octets whose body carries the
//!    counter every [`PERIOD`] octets, so that every buffer of a chain names
//!    its frame.

/// The rounds the sender sends, one frame of each case a round.
pub const ROUNDS: u32 = 12_000;
/// The QoS Data header: Frame Control, Duration, three addresses, Sequence
/// Control, QoS Control (IEEE 802.11-2020 9.3.2.1).
pub const QOS_HEADER: usize = 26;
/// A-MSDU subframe header: DA, SA, Length (IEEE 802.11-2020 9.3.2.2.2).
const SUBFRAME_HEADER: usize = 14;
/// The provenance series' counter period and its first place, after the
/// 24-octet header and the LLC/SNAP.
pub const PERIOD: usize = 64;
pub const BODY: usize = 32;
const SNAP: [u8; 8] = [0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0xb5];
/// The counter's place within an A-MSDU subframe: after its header and the
/// MSDU's LLC/SNAP.
const SUBFRAME_COUNTER: usize = SUBFRAME_HEADER + 8;

/// One case of the round.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Case {
    Amsdu2,
    Amsdu1,
    Control,
    AmsduLong,
    Provenance700,
    Provenance1500,
}

/// The cases in the order a round sends them.
pub const CASES: [Case; 6] = [
    Case::Amsdu2,
    Case::Amsdu1,
    Case::Control,
    Case::AmsduLong,
    Case::Provenance700,
    Case::Provenance1500,
];

/// The order of round `round`'s cases: a permutation drawn from the round,
/// so that every case follows every other (`tasks/160`: the bit or the
/// place after a long frame).
pub fn order(round: u32) -> [Case; 6] {
    let mut cases = CASES;
    let mut state = round.wrapping_add(1).wrapping_mul(0x9e37_79b9) | 1;
    for last in (1..cases.len()).rev() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let pick = (state as usize) % (last + 1);
        if let (Some(&a), Some(&b)) = (cases.get(last), cases.get(pick)) {
            if let Some(cell) = cases.get_mut(last) {
                *cell = b;
            }
            if let Some(cell) = cases.get_mut(pick) {
                *cell = a;
            }
        }
    }
    cases
}

/// The frame sent before `case`'s frame of round `round`; `None` for the
/// run's first.
pub fn before(round: u32, case: Case) -> Option<Case> {
    let this = order(round);
    match this.iter().position(|each| *each == case)? {
        0 => order(round.checked_sub(1)?).last().copied(),
        at => this.get(at - 1).copied(),
    }
}

/// An index for [`before`]'s answer: a case's place in [`CASES`], or
/// `CASES.len()` for none.
pub fn before_index(round: u32, case: Case) -> usize {
    before(round, case)
        .and_then(|before| CASES.iter().position(|each| *each == before))
        .unwrap_or(CASES.len())
}

impl Case {
    pub const fn tag(self) -> u32 {
        match self {
            Self::Amsdu2 => 1,
            Self::Amsdu1 => 2,
            Self::Control => 3,
            Self::AmsduLong => 4,
            Self::Provenance700 => 5,
            Self::Provenance1500 => 6,
        }
    }

    pub fn from_counter(counter: u32) -> Option<Self> {
        CASES.into_iter().find(|case| case.tag() == counter >> 28)
    }

    /// The round of a counter.
    pub const fn round(counter: u32) -> u32 {
        counter & 0x0fff_ffff
    }

    pub const fn counter(self, round: u32) -> u32 {
        self.tag() << 28 | round
    }

    /// Whether the frame is QoS Data (cases 1 to 4).
    pub const fn qos(self) -> bool {
        !matches!(self, Self::Provenance700 | Self::Provenance1500)
    }

    /// An MSDU's octets (its LLC/SNAP, the counter, its payload).
    const fn msdu(self) -> usize {
        match self {
            Self::AmsduLong => 600,
            _ => 20,
        }
    }

    const fn subframes(self) -> usize {
        match self {
            Self::Amsdu1 => 1,
            _ => 2,
        }
    }

    /// The MPDU's length.
    pub const fn length(self) -> usize {
        match self {
            Self::Provenance700 => 700,
            Self::Provenance1500 => 1_500,
            _ => {
                let last = SUBFRAME_HEADER + self.msdu();
                QOS_HEADER + (self.subframes() - 1) * last.next_multiple_of(4) + last
            }
        }
    }

    /// Where the frame's counter first is.
    pub const fn counter_at(self) -> usize {
        if self.qos() {
            QOS_HEADER + SUBFRAME_COUNTER
        } else {
            BODY
        }
    }

    /// Whether `offset` is a place of the counter.
    pub const fn counter_place(self, offset: usize) -> bool {
        if self.qos() {
            match self.subframe(offset) {
                Some((_, at)) => at >= SUBFRAME_COUNTER && at < SUBFRAME_COUNTER + 4,
                None => false,
            }
        } else {
            offset >= BODY && (offset - BODY) % PERIOD < 4
        }
    }

    /// The subframe of a QoS frame's `offset`, and the offset within it.
    const fn subframe(self, offset: usize) -> Option<(u8, usize)> {
        if offset < QOS_HEADER {
            return None;
        }
        let at = offset - QOS_HEADER;
        let padded = (SUBFRAME_HEADER + self.msdu()).next_multiple_of(4);
        if self.subframes() == 1 || at < padded {
            Some((0, at))
        } else {
            Some((1, at - padded))
        }
    }
}

/// The byte at `offset` of `case`'s frame `counter` from `sender` to
/// `receiver`. Duration, Sequence Control and the Retry flag are the MAC's.
pub fn byte(case: Case, sender: [u8; 6], receiver: [u8; 6], counter: u32, offset: usize) -> u8 {
    let counter = counter.to_le_bytes();
    let at = |bytes: &[u8], at: usize| bytes.get(at).copied().unwrap_or(0);
    match offset {
        0 => {
            if case.qos() {
                0x88
            } else {
                0x08
            }
        }
        4..10 => at(&receiver, offset - 4),
        10..16 => at(&sender, offset - 10),
        16..22 => at(&sender, offset - 16),
        // QoS Control: TID 0, A-MSDU Present (bit 7) but in the control.
        24 if case.qos() => {
            if matches!(case, Case::Control) {
                0
            } else {
                0x80
            }
        }
        _ if offset < 24 || (case.qos() && offset < QOS_HEADER) => 0,
        _ if !case.qos() => match offset {
            24..32 => at(&SNAP, offset - 24),
            _ if case.counter_place(offset) => at(&counter, (offset - BODY) % PERIOD),
            _ => offset as u8,
        },
        _ => match case.subframe(offset) {
            Some((index, inner)) => {
                let length = (case.msdu() as u16).to_be_bytes();
                match inner {
                    0..6 => at(&receiver, inner),
                    6..12 => at(&sender, inner - 6),
                    12..14 => at(&length, inner - 12),
                    14..22 => at(&SNAP, inner - 14),
                    _ if case.counter_place(offset) => at(&counter, inner - SUBFRAME_COUNTER),
                    _ if inner < SUBFRAME_HEADER + case.msdu() => (inner as u8) ^ (index << 7),
                    // The first subframe's padding to a word.
                    _ => 0,
                }
            }
            None => 0,
        },
    }
}

/// Cell E of `tasks/161`: the same QoS cases protected under CCMP. The
/// sender seals them in software (`CcmpAssociation`, SPP not negotiated, as
/// the access point of 2026-11-01); the receiver's MAC decrypts them under
/// the same pairwise key installed through the HAL.
/// Cell F's control: the frames of the aggregates of 2 subframes, then those
/// of the aggregates of 8, numbered in one run.
pub const CELL_F_PAIRS: u32 = 600;
pub const CELL_F_OCTETS: u32 = 150;
pub const CELL_F_FRAMES: u32 = 2 * CELL_F_PAIRS + 8 * CELL_F_OCTETS;
/// Cell F's A-MSDU stage: each A-MSDU case's frames, half in aggregates of 2
/// and half in aggregates of 8 (the long one in aggregates of 2 alone: eight
/// would outlast an HT MCS0 PPDU).
pub const CELL_F_AMSDU_FRAMES: u32 = 1_200;
/// The frames cell F sends of `case`.
pub const fn cell_f_frames(case: Case) -> u32 {
    match case {
        Case::Control => CELL_F_FRAMES,
        Case::Amsdu1 | Case::Amsdu2 | Case::AmsduLong => CELL_F_AMSDU_FRAMES,
        Case::Provenance700 | Case::Provenance1500 => 0,
    }
}
/// Cell G's control: single MPDUs sealed by the sender's MAC, of each of
/// these, before the aggregates.
pub const CELL_G_SINGLES: u32 = 200;
/// The frames cell G sends of `case`: its singles first (the control and
/// the A-MSDU of two short subframes), then cell F's aggregates, numbered on.
pub const fn cell_g_frames(case: Case) -> u32 {
    match case {
        Case::Control | Case::Amsdu2 => CELL_G_SINGLES + cell_f_frames(case),
        _ => cell_f_frames(case),
    }
}
/// Cell E's rounds: at least 1 000 frames of each QoS case.
pub const CELL_ROUNDS: u32 = 1_100;
pub const CCMP_HEADER: usize = 8;
pub const CCMP_MIC: usize = 8;
/// Frame Control's Protected Frame flag (IEEE 802.11-2020 9.2.4.1.9).
pub const PROTECTED: u8 = 0x40;
/// The cell's temporal key, shared by both probes; a probe's own constant,
/// protecting nothing.
pub const TEMPORAL_KEY: [u8; 16] = *b"oer-probe-cell-e";

impl Case {
    /// The protected MPDU's length: the CCMP header and MIC around the
    /// plaintext's.
    pub const fn protected_length(self) -> usize {
        self.length() + CCMP_HEADER + CCMP_MIC
    }

    /// Where the protected frame's counter first is.
    pub const fn protected_counter_at(self) -> usize {
        self.counter_at() + CCMP_HEADER
    }
}

/// The byte at `offset` of `case`'s protected frame before sealing: the
/// header with the Protected flag, the CCMP header of packet number `pn`
/// (key 0, ExtIV), the plaintext body, then the MIC's room as zeros.
pub fn protected_byte(
    case: Case,
    sender: [u8; 6],
    receiver: [u8; 6],
    counter: u32,
    pn: u64,
    offset: usize,
) -> u8 {
    let pn = pn.to_le_bytes();
    let at = |bytes: &[u8], at: usize| bytes.get(at).copied().unwrap_or(0);
    match offset {
        1 => byte(case, sender, receiver, counter, 1) | PROTECTED,
        _ if offset < QOS_HEADER => byte(case, sender, receiver, counter, offset),
        _ => match offset - QOS_HEADER {
            0 => at(&pn, 0),
            1 => at(&pn, 1),
            2 => 0,
            // ExtIV set, key ID 0 (IEEE 802.11-2020 12.5.3.2).
            3 => 0x20,
            inner @ 4..8 => at(&pn, inner - 2),
            _ if offset < QOS_HEADER + CCMP_HEADER + case.length() - QOS_HEADER => {
                byte(case, sender, receiver, counter, offset - CCMP_HEADER)
            }
            _ => 0,
        },
    }
}

/// What a receiver that decrypted `case`'s frame holds at `offset`, or
/// `None` where the packet number or the MIC lie.
pub fn protected_expected(
    case: Case,
    sender: [u8; 6],
    receiver: [u8; 6],
    counter: u32,
    offset: usize,
) -> Option<u8> {
    let header = QOS_HEADER..QOS_HEADER + CCMP_HEADER;
    if offset >= case.protected_length() - CCMP_MIC
        || (header.contains(&offset) && offset != QOS_HEADER + 2 && offset != QOS_HEADER + 3)
    {
        return None;
    }
    Some(protected_byte(case, sender, receiver, counter, 0, offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENDER: [u8; 6] = [1, 2, 3, 4, 5, 6];
    const RECEIVER: [u8; 6] = [9, 8, 7, 6, 5, 4];

    fn frame(case: Case, round: u32) -> Vec<u8> {
        (0..case.length())
            .map(|offset| byte(case, SENDER, RECEIVER, case.counter(round), offset))
            .collect()
    }

    #[test]
    fn every_counter_place_names_the_frame() {
        for case in CASES {
            let counter = case.counter(77);
            let bytes = frame(case, 77);
            let places: Vec<usize> = (0..case.length())
                .filter(|offset| case.counter_place(*offset))
                .collect();
            assert_eq!(places.len() % 4, 0);
            for start in places.chunks(4) {
                let at = start[0];
                assert_eq!(bytes[at..at + 4], counter.to_le_bytes());
            }
            let first = case.counter_at();
            let read = u32::from_le_bytes(bytes[first..first + 4].try_into().unwrap());
            assert!(Case::from_counter(read) == Some(case));
            assert_eq!(Case::round(read), 77);
        }
        // The provenance series names its frame in every 64 octets.
        let places = (0..1_500)
            .filter(|offset| Case::Provenance1500.counter_place(*offset))
            .count();
        assert_eq!(places, 4 * (1_500 - BODY).div_ceil(PERIOD));
    }

    #[test]
    fn every_case_follows_every_other() {
        let mut seen = [[0_u32; 7]; 6];
        for round in 0..ROUNDS {
            let order = order(round);
            for case in CASES {
                assert_eq!(order.iter().filter(|each| **each == case).count(), 1);
                let at = CASES.iter().position(|each| *each == case).unwrap();
                seen[at][before_index(round, case)] += 1;
            }
        }
        for (at, row) in seen.iter().enumerate() {
            for (before, count) in row.iter().enumerate().take(6) {
                if before != at {
                    assert!(*count > 300, "case {at} after {before}: {count}");
                }
            }
        }
    }

    #[test]
    fn the_control_differs_from_the_aggregate_only_in_its_bit() {
        assert_eq!(Case::Control.length(), Case::Amsdu2.length());
        let aggregate = frame(Case::Amsdu2, 5);
        let control = frame(Case::Control, 5);
        let differ: Vec<usize> = (0..aggregate.len())
            .filter(|at| aggregate[*at] != control[*at])
            .collect();
        // The QoS Control's A-MSDU Present bit and the counters' tags.
        assert!(differ.contains(&24));
        assert!(
            differ
                .iter()
                .all(|at| *at == 24 || Case::Control.counter_place(*at))
        );
    }

    #[test]
    fn a_protected_frame_is_the_plain_one_behind_its_ccmp_header() {
        for case in CASES.into_iter().filter(|case| case.qos()) {
            let counter = case.counter(9);
            let plain = frame(case, 9);
            let protected: Vec<u8> = (0..case.protected_length())
                .map(|offset| {
                    protected_byte(case, SENDER, RECEIVER, counter, 0x0102_0304_0506, offset)
                })
                .collect();
            assert_eq!(protected[1], plain[1] | PROTECTED);
            assert_eq!(protected[..QOS_HEADER][2..], plain[..QOS_HEADER][2..]);
            assert_eq!(
                protected[QOS_HEADER..QOS_HEADER + CCMP_HEADER],
                [6, 5, 0, 0x20, 4, 3, 2, 1]
            );
            assert_eq!(
                protected[QOS_HEADER + CCMP_HEADER..case.protected_length() - CCMP_MIC],
                plain[QOS_HEADER..]
            );
            let at = case.protected_counter_at();
            assert_eq!(protected[at..at + 4], counter.to_le_bytes());
            assert!(protected_expected(case, SENDER, RECEIVER, counter, QOS_HEADER).is_none());
            assert!(
                protected_expected(case, SENDER, RECEIVER, counter, case.protected_length() - 1)
                    .is_none()
            );
        }
    }

    #[test]
    fn the_subframes_parse_as_an_a_msdu() {
        for case in [Case::Amsdu2, Case::Amsdu1, Case::AmsduLong] {
            let bytes = frame(case, 1);
            let mut at = QOS_HEADER;
            let mut subframes = 0;
            while at < bytes.len() {
                assert_eq!(bytes[at..at + 6], RECEIVER);
                assert_eq!(bytes[at + 6..at + 12], SENDER);
                let length = usize::from(u16::from_be_bytes([bytes[at + 12], bytes[at + 13]]));
                assert_eq!(bytes[at + 14..at + 22], SNAP);
                at += (SUBFRAME_HEADER + length).next_multiple_of(4);
                subframes += 1;
            }
            assert_eq!(subframes, case.subframes());
        }
        // The long aggregate is longer than a 256-octet buffer.
        assert!(Case::AmsduLong.length() + 64 > 2 * 256);
        assert!(Case::Amsdu1.length() + 64 <= 128);
    }
}
