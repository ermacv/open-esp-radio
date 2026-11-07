//! The Internet checksum.
//!
//! SOURCE: RFC 1071, 1 (Introduction) and 4.1 (C); RFC 768 for UDP's pseudo
//! header and its zero; RFC 791, 3.1 for IPv4's header.

/// The one's complement sum of a sequence of octets, in any number of parts.
///
/// A part may end on an odd octet: the next part continues the sequence.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Checksum {
    /// Sums 16-bit words, so carries need 2^48 words to wrap; the wrap is
    /// named all the same.
    sum: u64,
    /// The first octet of a word whose second comes with the next part.
    odd: Option<u8>,
}

/// One's complement addition of 32-bit words: a carry comes back in.
const fn add_carrying(sum: u32, word: u32) -> u32 {
    let (added, carried) = sum.overflowing_add(word);
    added.wrapping_add(carried as u32)
}

impl Checksum {
    pub const fn new() -> Self {
        Self { sum: 0, odd: None }
    }

    /// Add the next octets of the sequence. This runs on every octet of every
    /// datagram, so what lies aligned goes as whole words, RFC 1071, 2 (B).
    #[must_use]
    pub fn with(mut self, bytes: &[u8]) -> Self {
        let (head, words, tail) = oer_portable_wire::aligned_words(bytes);
        self.octets(head);
        self.words(words);
        self.octets(tail);
        self
    }

    fn octets(&mut self, bytes: &[u8]) {
        for byte in bytes {
            match self.odd.take() {
                Some(high) => self.add_word(high, *byte),
                None => self.odd = Some(*byte),
            }
        }
    }

    /// Sum words in the machine's order, four running sums so a load and a
    /// carry stay apart. An octet waiting for its pair leaves the words one
    /// octet off the sequence's own: the sum is that of swapped words, and it
    /// meets the waiting octet at the end, as the sum commutes.
    fn words(&mut self, words: &[u32]) {
        let (mut first, mut second, mut third, mut fourth) = (0, 0, 0, 0);
        let mut rest = words;
        while let Some(([a, b, c, d], next)) = rest.split_first_chunk::<4>() {
            first = add_carrying(first, *a);
            second = add_carrying(second, *b);
            third = add_carrying(third, *c);
            fourth = add_carrying(fourth, *d);
            rest = next;
        }
        let mut sum = add_carrying(add_carrying(first, second), add_carrying(third, fourth));
        for word in rest {
            sum = add_carrying(sum, *word);
        }
        let folded = (sum & 0xFFFF).wrapping_add(sum >> 16);
        let folded = (folded & 0xFFFF).wrapping_add(folded >> 16) as u16;
        let ordered = u16::from_be(folded);
        let word = if self.odd.is_some() {
            ordered.swap_bytes()
        } else {
            ordered
        };
        self.sum = self.sum.wrapping_add(u64::from(word));
    }

    fn add_word(&mut self, high: u8, low: u8) {
        self.sum = self
            .sum
            .wrapping_add(u64::from(u16::from_be_bytes([high, low])));
    }

    /// The checksum field: the complement of the folded sum. A sequence ending
    /// on an odd octet counts it as the high octet of a last word.
    pub fn finish(mut self) -> u16 {
        if let Some(high) = self.odd.take() {
            self.add_word(high, 0);
        }
        let mut sum = self.sum;
        while sum > 0xFFFF {
            sum = (sum & 0xFFFF).wrapping_add(sum >> 16);
        }
        !(sum as u16)
    }

    /// Whether a sequence that holds its own checksum field sums to all ones.
    pub fn verifies(self) -> bool {
        self.finish() == 0
    }
}

/// UDP's field for `computed`: an all-zero result is sent as all ones, as
/// zero says that no checksum was computed.
pub const fn udp_field(computed: u16) -> u16 {
    if computed == 0 { 0xFFFF } else { computed }
}

/// The IPv4 pseudo header of a transport `length` octets long.
pub const fn ipv4_pseudo_header(
    source: [u8; 4],
    destination: [u8; 4],
    protocol: u8,
    length: u16,
) -> [u8; 12] {
    let [s0, s1, s2, s3] = source;
    let [d0, d1, d2, d3] = destination;
    let [length_high, length_low] = length.to_be_bytes();
    [
        s0,
        s1,
        s2,
        s3,
        d0,
        d1,
        d2,
        d3,
        0,
        protocol,
        length_high,
        length_low,
    ]
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::{vec, vec::Vec};

    use super::*;
    use crate::transport::PROTOCOL_UDP;

    /// RFC 1071, 3 (Numerical Examples): the sum 0xDDF2.
    const EXAMPLE: [u8; 8] = [0x00, 0x01, 0xF2, 0x03, 0xF4, 0xF5, 0xF6, 0xF7];

    #[test]
    fn the_rfc_example_gives_the_complement_of_its_sum() {
        assert_eq!(Checksum::new().with(&EXAMPLE).finish(), !0xDDF2);
    }

    /// RFC 1071's sum, one word at a time.
    fn reference(bytes: &[u8]) -> u16 {
        let mut sum: u64 = bytes
            .chunks(2)
            .map(|pair| match *pair {
                [high, low] => u64::from(u16::from_be_bytes([high, low])),
                [high] => u64::from(high) << 8,
                _ => 0,
            })
            .sum();
        while sum > 0xFFFF {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        !(sum as u16)
    }

    fn octets(length: usize) -> Vec<u8> {
        (0..length).map(|at| (at * 31 + (at >> 8)) as u8).collect()
    }

    #[test]
    fn any_length_sums_as_the_rfc_does() {
        for length in (0..80).chain([1471, 1472, 1473, 1500]) {
            let bytes = octets(length);
            assert_eq!(Checksum::new().with(&bytes).finish(), reference(&bytes));
        }
    }

    #[test]
    fn a_sequence_at_any_alignment_sums_as_the_rfc_does() {
        let bytes = octets(64);
        for start in 0..8 {
            for end in start..bytes.len() {
                let part = &bytes[start..end];
                assert_eq!(Checksum::new().with(part).finish(), reference(part));
            }
        }
    }

    #[test]
    fn parts_at_any_alignment_meet_on_an_odd_octet() {
        let bytes = octets(96);
        for first in 0..17 {
            for second in first..40 {
                let (head, rest) = bytes.split_at(first);
                let (middle, tail) = rest.split_at(second - first);
                let sum = Checksum::new().with(head).with(middle).with(tail);
                assert_eq!(sum.finish(), reference(&bytes), "at {first} and {second}");
            }
        }
    }

    #[test]
    fn empty_parts_change_nothing() {
        let bytes = octets(9);
        let (head, tail) = bytes.split_at(3);
        let sum = Checksum::new()
            .with(&[])
            .with(head)
            .with(&[])
            .with(tail)
            .with(&[]);
        assert_eq!(sum.finish(), reference(&bytes));
    }

    #[test]
    fn words_of_all_ones_carry_at_every_alignment() {
        let bytes = vec![0xFF; 4096];
        for start in 0..8 {
            let part = &bytes[start..];
            assert_eq!(Checksum::new().with(part).finish(), reference(part));
        }
    }

    #[test]
    fn a_sequence_longer_than_one_sum_loses_no_carry() {
        for length in [65_535, 65_536, 65_537, 200_001] {
            let bytes = vec![0xFF; length];
            assert_eq!(Checksum::new().with(&bytes).finish(), reference(&bytes));
        }
    }

    #[test]
    fn parts_of_any_length_meet_on_an_odd_octet() {
        let bytes = octets(301);
        for first in 0..bytes.len() {
            for second in [0, 1, 2, 3, 33] {
                let (head, rest) = bytes.split_at(first);
                let (middle, tail) = rest.split_at(second.min(rest.len()));
                let sum = Checksum::new().with(head).with(middle).with(tail);
                assert_eq!(sum.finish(), reference(&bytes), "at {first} and {second}");
            }
        }
    }

    #[test]
    fn a_split_at_any_octet_changes_nothing() {
        let whole = Checksum::new().with(&EXAMPLE).finish();
        for split in 0..=EXAMPLE.len() {
            let (first, second) = EXAMPLE.split_at(split);
            assert_eq!(Checksum::new().with(first).with(second).finish(), whole);
        }
        let single = EXAMPLE.iter().fold(Checksum::new(), |sum, byte| {
            sum.with(core::slice::from_ref(byte))
        });
        assert_eq!(single.finish(), whole);
    }

    #[test]
    fn an_odd_sequence_pads_its_last_octet_low() {
        assert_eq!(
            Checksum::new().with(&[0x12, 0x34, 0x56]).finish(),
            Checksum::new().with(&[0x12, 0x34, 0x56, 0x00]).finish()
        );
    }

    #[test]
    fn an_ipv4_header_verifies_with_its_own_field() {
        // 192.168.0.1 to 192.168.0.199, UDP; its field computed by hand.
        let header = [
            0x45, 0x00, 0x00, 0x73, 0x00, 0x00, 0x40, 0x00, 0x40, 0x11, 0x00, 0x00, 0xC0, 0xA8,
            0x00, 0x01, 0xC0, 0xA8, 0x00, 0xC7,
        ];
        assert_eq!(Checksum::new().with(&header).finish(), 0xB861);
        let mut filled = header;
        filled[10..12].copy_from_slice(&0xB861_u16.to_be_bytes());
        assert!(Checksum::new().with(&filled).verifies());
    }

    #[test]
    fn a_long_sequence_of_ones_folds_all_its_carries() {
        let ones = [0xFF; 4096];
        assert_eq!(Checksum::new().with(&ones).finish(), 0);
    }

    #[test]
    fn a_carry_that_a_first_fold_makes_is_folded_too() {
        // 0x1FFFF folds to 0x10000, then to 1.
        assert_eq!(
            Checksum::new()
                .with(&[0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x01])
                .finish(),
            !1
        );
    }

    #[test]
    fn a_udp_datagram_covers_its_pseudo_header() {
        let mut datagram = [0x12, 0x34, 0x56, 0x78, 0, 10, 0, 0, 0xAB, 0xCD];
        let pseudo = ipv4_pseudo_header([192, 0, 2, 1], [192, 0, 2, 2], PROTOCOL_UDP, 10);
        let field = Checksum::new().with(&pseudo).with(&datagram).finish();
        datagram[6..8].copy_from_slice(&udp_field(field).to_be_bytes());
        assert!(Checksum::new().with(&pseudo).with(&datagram).verifies());
    }

    #[test]
    fn a_computed_zero_goes_as_all_ones() {
        assert_eq!(udp_field(0), 0xFFFF);
        assert_eq!(udp_field(1), 1);
    }
}
