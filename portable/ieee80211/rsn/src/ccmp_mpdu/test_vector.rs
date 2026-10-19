//! Independent management vector shared by the MPDU transform and replay wrapper.

/// SA Query Response, Retry, sequence 0x123 fragment 5, PN 0xa1, key 00..0f.
/// Encrypted independently with Python cryptography AESCCM.
const MANAGEMENT_WIRE: &str =
    "d04800000200000000010200000000020200000000023512a10000200000000012714ee3c7f2391ea46ccabd";
pub(crate) const MANAGEMENT_BODY: [u8; 4] = [8, 1, 0x12, 0x34];

pub(crate) fn management_wire() -> std::vec::Vec<u8> {
    MANAGEMENT_WIRE
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
