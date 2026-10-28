#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

//! Stateless Bluetooth LE ACL AES-CCM frame protection and session-key transforms.
//!
//! Callers supply the IV, packet counter and direction. Connection procedures,
//! counter allocation and replay state belong to LL or the backend caller.
//! Key material and RustCrypto AES key schedules are zeroized on drop.

use oer_bluetooth_le_wire::data_header::{HeaderFlags, ReceivedHeader, SequenceNumber};

use aes::{
    Aes128,
    cipher::{BlockEncrypt, KeyInit},
};

use ccm::{
    Ccm,
    aead::AeadInPlace,
    consts::{U4, U13},
};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Four-octet MIC appended to every encrypted ACL Data Physical Channel PDU.
pub const LE_ACL_MIC_BYTES: usize = 4;
/// Largest plaintext in an encrypted legacy 27-octet Data PDU.
pub const LE_LEGACY_ENCRYPTED_PLAINTEXT_BYTES: usize = 27 - LE_ACL_MIC_BYTES;
/// Largest plaintext whose encrypted payload and MIC fit the one-octet Length.
pub const LE_ENCRYPTED_PLAINTEXT_BYTES: usize = u8::MAX as usize - LE_ACL_MIC_BYTES;
/// Largest admitted 39-bit packet counter supplied to the nonce transform.
pub const LE_PACKET_COUNTER_MAX: u64 = (1 << 39) - 1;

/// Host-provided 128-bit Long Term Key.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct LeLongTermKey([u8; 16]);

impl LeLongTermKey {
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Derive the connection session key as `AES-128(LTK, SKD)`.
    pub fn derive_session_key(&self, diversifier: LeSessionKeyDiversifier) -> LeAclSessionKey {
        let mut aes_key = Zeroizing::new(self.0);
        aes_key.reverse();
        let cipher = Aes128::new((&*aes_key).into());
        let mut key = Zeroizing::new(diversifier.0);
        key.reverse();
        encrypt_block(&cipher, &mut key);
        key.reverse();
        LeAclSessionKey(*key)
    }
}

/// Combined Central and Peripheral session-key diversifier in wire octet order.
pub struct LeSessionKeyDiversifier([u8; 16]);

impl LeSessionKeyDiversifier {
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

/// Combined Central and Peripheral initialization vector in nonce octet order.
pub struct LeAclInitializationVector([u8; 8]);

impl LeAclInitializationVector {
    pub const fn new(bytes: [u8; 8]) -> Self {
        Self(bytes)
    }
}

/// Session key produced from one LTK and one connection-specific SKD.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct LeAclSessionKey([u8; 16]);

/// Failure before a packet can cross the encrypted Link Layer boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeAclEncryptionError {
    BufferTooSmall,
    /// Only an LLID=01 zero-length Data PDU can bypass CCM.
    InvalidEmptyPdu,
    PayloadTooLong,
    /// The supplied counter does not fit 39 bits, or the LL counter owner is exhausted.
    PacketCounterExhausted,
    MicMismatch,
}

impl LeAclInitializationVector {
    /// Build the Core Vol 6 Part E nonce from a supplied 39-bit counter.
    /// `central` selects Central-to-Peripheral traffic; false selects the reverse.
    pub fn nonce(&self, counter: u64, central: bool) -> Result<[u8; 13], LeAclEncryptionError> {
        nonce(&self.0, counter, central)
    }
}

/// Encrypt one payload in place and append its MIC using caller-selected
/// counter, direction and IV. This transform never allocates or advances a counter.
/// `central` selects Central-to-Peripheral traffic.
/// Empty LLID=01 PDUs return zero without touching storage or using a nonce.
pub fn encrypt_packet(
    key: &LeAclSessionKey,
    initialization_vector: &LeAclInitializationVector,
    counter: u64,
    central: bool,
    header: u8,
    packet: &mut [u8],
    plaintext_len: usize,
) -> Result<usize, LeAclEncryptionError> {
    let encrypted_len = encrypted_packet_length(header, packet.len(), plaintext_len)?;
    if encrypted_len == 0 {
        return Ok(0);
    }
    encrypt_ccm(
        &key.0,
        &initialization_vector.0,
        counter,
        central,
        header,
        packet,
        plaintext_len,
    )?;
    Ok(encrypted_len)
}

/// Authenticate and decrypt one payload, including its MIC, in place.
/// On MIC failure the complete supplied payload is zeroized. On success
/// the MIC storage is cleared. Empty LLID=01 PDUs bypass CCM; MIC-only
/// payloads are refused. Counter allocation and replay checks belong to the caller.
pub fn decrypt_packet(
    key: &LeAclSessionKey,
    initialization_vector: &LeAclInitializationVector,
    counter: u64,
    central: bool,
    header: u8,
    packet: &mut [u8],
) -> Result<usize, LeAclEncryptionError> {
    let plaintext_len = decrypted_packet_length(header, packet.len())?;
    if packet.is_empty() {
        return Ok(0);
    }
    if let Err(error) = decrypt_ccm(
        &key.0,
        &initialization_vector.0,
        counter,
        central,
        header,
        packet,
        plaintext_len,
    ) {
        if error == LeAclEncryptionError::MicMismatch {
            packet.zeroize();
        }
        return Err(error);
    }
    packet
        .get_mut(plaintext_len..)
        .ok_or(LeAclEncryptionError::BufferTooSmall)?
        .fill(0);
    Ok(plaintext_len)
}

/// Validate a plaintext extent before a caller allocates a packet counter.
/// Returns the payload length including the MIC, or zero for an Empty PDU.
pub fn encrypted_packet_length(
    header: u8,
    buffer_len: usize,
    plaintext_len: usize,
) -> Result<usize, LeAclEncryptionError> {
    if plaintext_len == 0 {
        return empty_pdu_length(header);
    }
    validate_encrypt_buffer(buffer_len, plaintext_len)
}

/// Validate a received payload extent before a caller selects its packet counter.
/// Returns the plaintext length, or zero for an Empty PDU.
pub fn decrypted_packet_length(
    header: u8,
    encrypted_len: usize,
) -> Result<usize, LeAclEncryptionError> {
    if encrypted_len == 0 {
        return empty_pdu_length(header);
    }
    if encrypted_len <= LE_ACL_MIC_BYTES {
        return Err(LeAclEncryptionError::BufferTooSmall);
    }
    let plaintext_len = encrypted_len
        .checked_sub(LE_ACL_MIC_BYTES)
        .ok_or(LeAclEncryptionError::BufferTooSmall)?;
    if plaintext_len > LE_ENCRYPTED_PLAINTEXT_BYTES {
        return Err(LeAclEncryptionError::PayloadTooLong);
    }
    Ok(plaintext_len)
}

fn empty_pdu_length(header: u8) -> Result<usize, LeAclEncryptionError> {
    if ReceivedHeader::new([header, 0]).empty {
        Ok(0)
    } else {
        Err(LeAclEncryptionError::InvalidEmptyPdu)
    }
}

fn validate_encrypt_buffer(
    buffer_len: usize,
    plaintext_len: usize,
) -> Result<usize, LeAclEncryptionError> {
    if plaintext_len > LE_ENCRYPTED_PLAINTEXT_BYTES {
        return Err(LeAclEncryptionError::PayloadTooLong);
    }
    let encrypted_len = plaintext_len
        .checked_add(LE_ACL_MIC_BYTES)
        .ok_or(LeAclEncryptionError::PayloadTooLong)?;
    if buffer_len < encrypted_len {
        return Err(LeAclEncryptionError::BufferTooSmall);
    }
    Ok(encrypted_len)
}

fn nonce(
    initialization_vector: &[u8; 8],
    counter: u64,
    central: bool,
) -> Result<[u8; 13], LeAclEncryptionError> {
    if counter > LE_PACKET_COUNTER_MAX {
        return Err(LeAclEncryptionError::PacketCounterExhausted);
    }
    let [a, b, c, d, mut e, _, _, _] = counter.to_le_bytes();
    e &= 0x7f;
    if central {
        e |= 0x80;
    }
    let [f, g, h, i, j, k, l, m] = *initialization_vector;
    Ok([a, b, c, d, e, f, g, h, i, j, k, l, m])
}

fn aad(header: u8) -> u8 {
    // NESN, SN and MD are authenticated as zero; LLID and CTEInfoPresent remain.
    HeaderFlags {
        sn: SequenceNumber::ZERO,
        nesn: SequenceNumber::ZERO,
        md: false,
    }
    .apply(header)
}

type LeAclCcm = Ccm<Aes128, U4, U13>;

fn encrypt_ccm(
    key: &[u8; 16],
    initialization_vector: &[u8; 8],
    counter: u64,
    central: bool,
    header: u8,
    packet: &mut [u8],
    plaintext_len: usize,
) -> Result<(), LeAclEncryptionError> {
    let nonce = nonce(initialization_vector, counter, central)?;
    let (plaintext, tail) = packet
        .split_at_mut_checked(plaintext_len)
        .ok_or(LeAclEncryptionError::BufferTooSmall)?;
    let (mic, _) = tail
        .split_first_chunk_mut::<LE_ACL_MIC_BYTES>()
        .ok_or(LeAclEncryptionError::BufferTooSmall)?;
    let mut aes_key = Zeroizing::new(*key);
    aes_key.reverse();
    let cipher = LeAclCcm::new((&*aes_key).into());
    let tag = cipher
        .encrypt_in_place_detached((&nonce).into(), &[aad(header)], plaintext)
        .map_err(|_| LeAclEncryptionError::PayloadTooLong)?;
    *mic = tag.into();
    Ok(())
}

fn decrypt_ccm(
    key: &[u8; 16],
    initialization_vector: &[u8; 8],
    counter: u64,
    central: bool,
    header: u8,
    packet: &mut [u8],
    plaintext_len: usize,
) -> Result<(), LeAclEncryptionError> {
    let nonce = nonce(initialization_vector, counter, central)?;
    let (plaintext, tail) = packet
        .split_at_mut_checked(plaintext_len)
        .ok_or(LeAclEncryptionError::BufferTooSmall)?;
    let mic = tail
        .first_chunk::<LE_ACL_MIC_BYTES>()
        .ok_or(LeAclEncryptionError::BufferTooSmall)?;
    let mut aes_key = Zeroizing::new(*key);
    aes_key.reverse();
    let cipher = LeAclCcm::new((&*aes_key).into());
    // RustCrypto compares the complete detached MIC in constant time.
    cipher
        .decrypt_in_place_detached((&nonce).into(), &[aad(header)], plaintext, mic.into())
        .map_err(|_| LeAclEncryptionError::MicMismatch)
}

fn encrypt_block(cipher: &Aes128, block: &mut [u8; 16]) {
    cipher.encrypt_block(block.into());
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod key_schedule_tests {
    #[test]
    fn aes_key_schedules_zeroize_on_drop() {
        fn assert_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}
        assert_zeroize_on_drop::<super::LeLongTermKey>();
        assert_zeroize_on_drop::<super::LeAclSessionKey>();
        assert_zeroize_on_drop::<aes::Aes128>();
        assert_zeroize_on_drop::<aes::Aes192>();
        assert_zeroize_on_drop::<aes::Aes256>();
    }
}
