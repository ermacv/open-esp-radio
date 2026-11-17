//! Case C's frames (`tasks/154`, with `159`'s provenance series): one layout
//! for the sender (`esp32s31-mac-reset --features amsdu`) and the receiver
//! (`esp32s31-rx-chain --features laps,amsdu`, which includes this file).
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
pub const ROUNDS: u32 = 2_600;
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
