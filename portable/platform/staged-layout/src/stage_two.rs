//! Stage-two image header and payload checksum.
//!
//! The runtime linker script emits the header at the start of the image
//! (from the symbols in [`crate::build`]), the host packer stores the payload
//! CRC and the bootstrap validates both before transferring control. The CRC
//! covers the whole payload with the checksum field read as zero.

use core::mem::{offset_of, size_of};

/// `"STG2"` in little-endian byte order.
pub const MAGIC: u32 = 0x3247_5453;

/// Bootstrap/runtime handoff ABI version.
pub const ABI_VERSION: u32 = 1;

/// Little-endian image header at the stage-two load address.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    pub magic: u32,
    pub abi_version: u32,
    /// Required load address of the image.
    pub load_address: u32,
    /// Inherited-stack entry point.
    pub entry: u32,
    /// End of the initialized payload.
    pub payload_end: u32,
    /// PSRAM BSS that the bootstrap may clear; empty when data stays in SRAM.
    pub bss_start: u32,
    pub bss_end: u32,
    pub header_size: u32,
    /// Executable range.
    pub text_start: u32,
    pub text_end: u32,
    pub payload_crc32: u32,
}

/// Size of [`Header`] in the image.
pub const HEADER_BYTES: usize = size_of::<Header>();

/// Byte offset of [`Header::payload_crc32`] in the image.
pub const CRC_OFFSET: usize = offset_of!(Header, payload_crc32);

const CRC_END: usize = CRC_OFFSET + size_of::<u32>();

impl Header {
    /// Reads a header from the start of an image.
    pub fn from_le_bytes(image: &[u8]) -> Option<Self> {
        let word = |index: usize| {
            let bytes = image.get(index * 4..index * 4 + 4)?;
            Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        };
        Some(Self {
            magic: word(0)?,
            abi_version: word(1)?,
            load_address: word(2)?,
            entry: word(3)?,
            payload_end: word(4)?,
            bss_start: word(5)?,
            bss_end: word(6)?,
            header_size: word(7)?,
            text_start: word(8)?,
            text_end: word(9)?,
            payload_crc32: word(10)?,
        })
    }

    /// Serializes the header as it appears in the image.
    pub fn to_le_bytes(&self) -> [u8; HEADER_BYTES] {
        let words = [
            self.magic,
            self.abi_version,
            self.load_address,
            self.entry,
            self.payload_end,
            self.bss_start,
            self.bss_end,
            self.header_size,
            self.text_start,
            self.text_end,
            self.payload_crc32,
        ];
        let mut bytes = [0; HEADER_BYTES];
        for (chunk, word) in bytes.chunks_exact_mut(4).zip(words) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }

    /// Whether the magic, ABI version and header size match this contract.
    pub const fn is_compatible(&self) -> bool {
        self.magic == MAGIC
            && self.abi_version == ABI_VERSION
            && self.header_size as usize == HEADER_BYTES
    }
}

/// Incremental CRC-32 (IEEE 802.3) over payload bytes in image order.
///
/// Bytes at the checksum field's offset are hashed as zero, so the stored
/// value does not affect its own calculation.
#[derive(Clone, Copy, Debug)]
pub struct PayloadCrc {
    state: u32,
    index: usize,
}

impl PayloadCrc {
    pub const fn new() -> Self {
        Self {
            state: 0xffff_ffff,
            index: 0,
        }
    }

    /// Hashes the next payload byte.
    pub fn push(&mut self, byte: u8) {
        let byte = if (CRC_OFFSET..CRC_END).contains(&self.index) {
            0
        } else {
            byte
        };
        self.index += 1;
        self.state ^= u32::from(byte);
        for _ in 0..8 {
            self.state = (self.state >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(self.state & 1));
        }
    }

    pub const fn finish(&self) -> u32 {
        !self.state
    }
}

impl Default for PayloadCrc {
    fn default() -> Self {
        Self::new()
    }
}

/// CRC of a complete payload held in memory.
pub fn payload_crc32(payload: &[u8]) -> u32 {
    let mut crc = PayloadCrc::new();
    payload.iter().for_each(|byte| crc.push(*byte));
    crc.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(payload_crc32: u32) -> Header {
        Header {
            magic: MAGIC,
            abi_version: ABI_VERSION,
            load_address: 0x1000,
            entry: 0x1040,
            payload_end: 0x2000,
            bss_start: 0x2000,
            bss_end: 0x2100,
            header_size: HEADER_BYTES as u32,
            text_start: 0x1030,
            text_end: 0x1800,
            payload_crc32,
        }
    }

    fn image(payload_crc32: u32) -> [u8; HEADER_BYTES + 3] {
        let mut image = [0xa5; HEADER_BYTES + 3];
        image[..HEADER_BYTES].copy_from_slice(&header(payload_crc32).to_le_bytes());
        image
    }

    #[test]
    fn stored_checksum_does_not_affect_the_payload_crc() {
        assert_eq!(payload_crc32(&image(0)), payload_crc32(&image(0xdead_beef)));
        let mut changed = image(0);
        changed[HEADER_BYTES] ^= 1;
        assert_ne!(payload_crc32(&image(0)), payload_crc32(&changed));
    }

    #[test]
    fn checksum_is_standard_crc32_for_bytes_outside_the_field() {
        // CRC-32/ISO-HDLC check value of "123456789".
        assert_eq!(payload_crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn header_round_trips_and_short_images_are_rejected() {
        let bytes = image(0x0102_0304);
        let parsed = Header::from_le_bytes(&bytes).unwrap();
        assert_eq!(parsed, header(0x0102_0304));
        assert!(parsed.is_compatible());
        assert_eq!(Header::from_le_bytes(&bytes[..HEADER_BYTES - 1]), None);
    }

    #[test]
    fn incompatible_headers_are_detected() {
        for change in [
            |h: &mut Header| h.magic ^= 1,
            |h: &mut Header| h.abi_version += 1,
            |h: &mut Header| h.header_size += 4,
        ] {
            let mut header = header(0);
            change(&mut header);
            assert!(!header.is_compatible());
        }
    }

    /// The value of the fixture line `key`.
    fn fixture(key: &str) -> &'static str {
        include_str!("../fixtures/stage-two.txt")
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(' '))
            .unwrap()
    }

    #[test]
    fn the_shared_packing_fixture_is_this_contract() {
        let hex = fixture("unpacked").as_bytes();
        let mut unpacked = [0_u8; 64];
        assert_eq!(hex.len(), unpacked.len() * 2);
        for (byte, pair) in unpacked.iter_mut().zip(hex.chunks_exact(2)) {
            let digit = |c: u8| (c as char).to_digit(16).unwrap() as u8;
            *byte = digit(pair[0]) << 4 | digit(pair[1]);
        }
        let crc = u32::from_str_radix(fixture("crc32"), 16).unwrap();
        let header = Header::from_le_bytes(&unpacked).unwrap();
        assert!(header.is_compatible());
        assert_eq!(header.payload_crc32, 0);
        assert_eq!(payload_crc32(&unpacked), crc);
        // The stored CRC does not change its own calculation.
        let mut packed = unpacked;
        packed[CRC_OFFSET..CRC_END].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(payload_crc32(&packed), crc);
    }
}
