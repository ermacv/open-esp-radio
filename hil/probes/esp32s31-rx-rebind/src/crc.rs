//! IEEE 802.3 CRC-32, the 802.11 FCS (IEEE 802.11-2020 9.2.4.8).

/// The reflected CRC-32 of IEEE 802.3 (`CRC-32/ISO-HDLC`).
pub(crate) const CRC32: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);

/// The CRC-32 of `bytes`.
pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    CRC32.checksum(bytes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_check_value_matches_the_standard() {
        assert_eq!(super::crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn one_changed_byte_changes_the_crc() {
        let mut bytes = [0x5a_u8; 64];
        let before = super::crc32(&bytes);
        if let Some(last) = bytes.last_mut() {
            *last ^= 1;
        }
        assert_ne!(super::crc32(&bytes), before);
    }

    #[test]
    fn a_digest_over_words_matches_the_whole_buffer() {
        let bytes: [u8; 16] = core::array::from_fn(|i| i as u8);
        let mut digest = super::CRC32.digest();
        for chunk in bytes.chunks_exact(4) {
            digest.update(chunk);
        }
        assert_eq!(digest.finalize(), super::crc32(&bytes));
    }
}
