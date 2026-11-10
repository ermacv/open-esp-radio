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

    /// Copy `source` to the start of `destination` and add it to the
    /// sequence: the aligned words of `source`, all but at most three octets
    /// at each edge, are read once, for their copy and their sum together;
    /// the edge octets are copied and summed apart. The sum is that of [`Self::with`] over
    /// `source`, a pending odd octet included, and `destination` holds
    /// `source`'s octets; the rest of `destination` is untouched.
    ///
    /// # Errors
    /// [`CopyShort`]: `destination` is shorter than `source`, and nothing
    /// was written.
    pub fn copied(mut self, source: &[u8], destination: &mut [u8]) -> Result<Self, CopyShort> {
        let destination = destination.get_mut(..source.len()).ok_or(CopyShort)?;
        let (head, words, tail) = oer_portable_wire::aligned_words(source);
        let (head_out, rest) = destination
            .split_at_mut_checked(head.len())
            .ok_or(CopyShort)?;
        copy(head, head_out);
        self.octets(head);
        let (words_out, tail_out) = rest
            .split_at_mut_checked(rest.len().saturating_sub(tail.len()))
            .ok_or(CopyShort)?;
        match oer_portable_wire::aligned_words_mut(words_out) {
            // Both sides at one offset in their words: whole words go.
            ([], aligned, []) if aligned.len() == words.len() => self.store_words(words, aligned),
            _ => self.copy_words(words, words_out)?,
        }
        copy(tail, tail_out);
        self.octets(tail);
        Ok(self)
    }

    /// Add `count` octets of `byte`, the next of the sequence, without
    /// reading them: the sum of [`Self::with`] over them, in time that does
    /// not grow with `count`. Each whole word of them is `byte` twice.
    #[must_use]
    pub fn repeated(mut self, byte: u8, count: usize) -> Self {
        let mut left = count;
        if left == 0 {
            return self;
        }
        if let Some(high) = self.odd.take() {
            self.add_word(high, byte);
            left = left.saturating_sub(1);
        }
        let words = u64::try_from(left >> 1).unwrap_or(u64::MAX);
        let word = u64::from(u16::from_be_bytes([byte, byte]));
        self.sum = self.sum.wrapping_add(words.wrapping_mul(word));
        if left & 1 == 1 {
            self.odd = Some(byte);
        }
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
        self.add_words(sum);
    }

    /// [`Self::words`], each word also stored to the word of `destination`
    /// at its place.
    fn store_words(&mut self, words: &[u32], destination: &mut [u32]) {
        let (mut first, mut second, mut third, mut fourth) = (0, 0, 0, 0);
        let mut rest = words;
        let mut out = destination;
        while let Some(([a, b, c, d], next)) = rest.split_first_chunk::<4>() {
            let Some((into, after)) = core::mem::take(&mut out).split_first_chunk_mut::<4>() else {
                break;
            };
            *into = [*a, *b, *c, *d];
            first = add_carrying(first, *a);
            second = add_carrying(second, *b);
            third = add_carrying(third, *c);
            fourth = add_carrying(fourth, *d);
            rest = next;
            out = after;
        }
        let mut sum = add_carrying(add_carrying(first, second), add_carrying(third, fourth));
        for (word, into) in rest.iter().zip(out) {
            *into = *word;
            sum = add_carrying(sum, *word);
        }
        self.add_words(sum);
    }

    /// [`Self::words`], each word also stored to `destination` in the
    /// machine's order, which is the octets it was loaded from:
    /// `destination` holds four octets a word, at any alignment.
    fn copy_words(&mut self, words: &[u32], destination: &mut [u8]) -> Result<(), CopyShort> {
        let (mut first, mut second, mut third, mut fourth) = (0, 0, 0, 0);
        let mut rest = words;
        let mut out = destination;
        while let Some(([a, b, c, d], next)) = rest.split_first_chunk::<4>() {
            let (into, after) = out.split_first_chunk_mut::<16>().ok_or(CopyShort)?;
            let [a0, a1, a2, a3] = a.to_ne_bytes();
            let [b0, b1, b2, b3] = b.to_ne_bytes();
            let [c0, c1, c2, c3] = c.to_ne_bytes();
            let [d0, d1, d2, d3] = d.to_ne_bytes();
            *into = [
                a0, a1, a2, a3, b0, b1, b2, b3, c0, c1, c2, c3, d0, d1, d2, d3,
            ];
            first = add_carrying(first, *a);
            second = add_carrying(second, *b);
            third = add_carrying(third, *c);
            fourth = add_carrying(fourth, *d);
            rest = next;
            out = after;
        }
        let mut sum = add_carrying(add_carrying(first, second), add_carrying(third, fourth));
        for word in rest {
            let (into, after) = out.split_first_chunk_mut::<4>().ok_or(CopyShort)?;
            *into = word.to_ne_bytes();
            sum = add_carrying(sum, *word);
            out = after;
        }
        self.add_words(sum);
        Ok(())
    }

    /// Add `sum`, the one's complement sum of words loaded in the machine's
    /// order.
    fn add_words(&mut self, sum: u32) {
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

/// The destination of a [`Checksum::copied`] does not hold its source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyShort;

/// `source` into `destination`, of the same length, octet by octet.
fn copy(source: &[u8], destination: &mut [u8]) {
    for (to, from) in destination.iter_mut().zip(source) {
        *to = *from;
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

    /// Repeated octets sum as the octets themselves, after a part of any
    /// length, odd or even, and before another.
    #[test]
    fn repeated_octets_sum_as_the_octets_written_out() {
        let head = octets(9);
        for before in 0..head.len() {
            for byte in [0x00, 0x5a, 0xff] {
                for count in (0..12).chain([1443, 1444, 1445]) {
                    let filler = vec![byte; count];
                    let tail = [0x12, 0x34, 0x56];
                    let mut whole = head[..before].to_vec();
                    whole.extend_from_slice(&filler);
                    whole.extend_from_slice(&tail);
                    let sum = Checksum::new()
                        .with(&head[..before])
                        .repeated(byte, count)
                        .with(&tail);
                    assert_eq!(
                        sum.finish(),
                        reference(&whole),
                        "{before} before, {count} of {byte:#x}"
                    );
                }
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

    /// A copy summed as it goes holds its source and sums as a sum of that
    /// source alone: any length, odd ones too, at any alignment of either
    /// side (both at one offset in their words, whole words go; else word
    /// loads and octet stores), after a part that left an odd octet pending
    /// or not, and leaves the rest of its destination as it was.
    #[test]
    fn a_copy_summed_as_it_goes_is_the_copy_and_the_sum() {
        let bytes = octets(160);
        let (mut together, mut apart) = (0, 0);
        for before in [0, 1, 2, 3, 7] {
            let prefix = &bytes[..before];
            for start in 0..5 {
                for length in (0..70).chain([127, 128, 129]) {
                    let source = &bytes[start..start + length];
                    for offset in 0..5 {
                        let mut destination = vec![0xA5; offset + length + 3];
                        if length >= 8 {
                            let lined_up = source.as_ptr().align_offset(4)
                                == destination[offset..].as_ptr().align_offset(4);
                            if lined_up {
                                together += 1;
                            } else {
                                apart += 1;
                            }
                        }
                        let sum = Checksum::new()
                            .with(prefix)
                            .copied(source, &mut destination[offset..])
                            .unwrap();
                        let expected = Checksum::new().with(prefix).with(source);
                        assert_eq!(sum, expected, "{before} {start} {length} {offset}");
                        assert_eq!(sum.finish(), expected.finish());
                        assert_eq!(&destination[offset..offset + length], source);
                        assert!(destination[..offset].iter().all(|byte| *byte == 0xA5));
                        assert!(
                            destination[offset + length..]
                                .iter()
                                .all(|byte| *byte == 0xA5)
                        );
                        let mut reference_sum = prefix.to_vec();
                        reference_sum.extend_from_slice(source);
                        assert_eq!(sum.finish(), reference(&reference_sum));
                    }
                }
            }
        }
        assert!(together > 0 && apart > 0, "both ways of storing words");
    }

    /// A destination too short for its source is refused untouched.
    #[test]
    fn a_copy_into_too_little_room_writes_nothing() {
        let source = octets(9);
        let mut destination = [0x5A; 8];
        assert_eq!(
            Checksum::new().copied(&source, &mut destination),
            Err(CopyShort)
        );
        assert_eq!(destination, [0x5A; 8]);
    }

    /// A summed copy may be followed by more parts, as any part may.
    #[test]
    fn a_summed_copy_is_one_part_among_others() {
        let bytes = octets(41);
        let (head, rest) = bytes.split_at(5);
        let (middle, tail) = rest.split_at(17);
        let mut room = [0; 17];
        let sum = Checksum::new()
            .with(head)
            .copied(middle, &mut room)
            .unwrap()
            .with(tail);
        assert_eq!(sum.finish(), reference(&bytes));
        assert_eq!(room, middle);
    }

    #[test]
    fn a_computed_zero_goes_as_all_ones() {
        assert_eq!(udp_field(0), 0xFFFF);
        assert_eq!(udp_field(1), 1);
    }
}
