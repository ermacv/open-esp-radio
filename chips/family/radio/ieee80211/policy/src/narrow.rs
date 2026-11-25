//! The vendor's own narrowings, each named for the instructions that make it.

/// What a byte store (`sb`) or an `andi …, 255` of the vendor's keeps of a
/// word: its low octet.
pub(crate) const fn low_octet(word: u32) -> u8 {
    let [low, ..] = word.to_le_bytes();
    low
}

/// What a halfword store (`sh`) of the vendor's keeps of a word: its low
/// sixteen bits.
pub(crate) const fn low_halfword(word: i64) -> u16 {
    let [low, high, ..] = word.to_le_bytes();
    u16::from_le_bytes([low, high])
}
