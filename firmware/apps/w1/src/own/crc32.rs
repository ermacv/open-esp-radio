//! A frame fingerprint: IEEE CRC-32, with one volatile read per byte.
const CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);

pub(super) fn crc32(bytes: &[u8]) -> u32 {
    let mut digest = CRC.digest();
    for byte in bytes {
        let byte = oer_memory::read_volatile(byte);
        digest.update(core::slice::from_ref(&byte));
    }
    digest.finalize()
}

#[cfg(test)]
mod tests {
    use super::crc32;
    #[test]
    fn frame_fingerprints_keep_ieee_crc_and_observe_changed_storage() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        let mut bytes = *b"123456789";
        let first = crc32(&bytes);
        bytes[0] = b'0';
        assert_ne!(crc32(&bytes), first);
        bytes[0] = b'1';
        assert_eq!(crc32(&bytes), first);
    }
}
