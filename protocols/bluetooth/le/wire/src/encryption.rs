//! Encryption control body codecs (Core Vol 6 Part B 2.4.2).
//!
//! Key ownership, AES-CCM transforms and procedure state live above wire.
use oer_wire::PrefixLength;

/// Rand and EDIV copied from one admitted Central `LL_ENC_REQ`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLongTermKeyRequest {
    random_number: [u8; 8],
    encrypted_diversifier: u16,
}

impl LeLongTermKeyRequest {
    pub const fn random_number(self) -> [u8; 8] {
        self.random_number
    }

    pub const fn encrypted_diversifier(self) -> u16 {
        self.encrypted_diversifier
    }
}

/// One retained LL Control response owned by the encryption procedure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeEncryptionControlResponse {
    bytes: [u8; 13],
    len: PrefixLength<13>,
    encrypted: bool,
}

impl LeEncryptionControlResponse {
    pub fn as_bytes(&self) -> &[u8] {
        self.len.borrow(&self.bytes)
    }

    pub const fn is_encrypted(self) -> bool {
        self.encrypted
    }
}

impl LeLongTermKeyRequest {
    pub const fn new(random_number: [u8; 8], encrypted_diversifier: u16) -> Self {
        Self {
            random_number,
            encrypted_diversifier,
        }
    }
}
/// Fields of one exact LL_ENC_REQ, before procedure admission.
pub struct LeEncryptionRequest {
    pub key: LeLongTermKeyRequest,
    pub diversifier: [u8; 8],
    pub initialization_vector: [u8; 4],
}
impl LeEncryptionRequest {
    pub fn decode(request: &[u8]) -> Option<Self> {
        let request: &[u8; 23] = request.try_into().ok()?;
        let [
            opcode,
            r0,
            r1,
            r2,
            r3,
            r4,
            r5,
            r6,
            r7,
            lo,
            hi,
            s0,
            s1,
            s2,
            s3,
            s4,
            s5,
            s6,
            s7,
            i0,
            i1,
            i2,
            i3,
        ] = *request;
        if opcode != 0x03 {
            return None;
        }
        Some(Self {
            key: LeLongTermKeyRequest::new(
                [r0, r1, r2, r3, r4, r5, r6, r7],
                u16::from_le_bytes([lo, hi]),
            ),
            diversifier: [s0, s1, s2, s3, s4, s5, s6, s7],
            initialization_vector: [i0, i1, i2, i3],
        })
    }
}
impl LeEncryptionControlResponse {
    pub fn encryption_response(diversifier: [u8; 16], initialization_vector: [u8; 8]) -> Self {
        let mut bytes = [0; 13];
        for (slot, byte) in bytes.iter_mut().zip(
            core::iter::once(0x04)
                .chain(diversifier.into_iter().skip(8))
                .chain(initialization_vector.into_iter().skip(4)),
        ) {
            *slot = byte;
        }
        Self {
            bytes,
            len: PrefixLength::constant::<13>(),
            encrypted: false,
        }
    }
    pub const fn start_request() -> Self {
        Self {
            bytes: [0x05, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            len: PrefixLength::constant::<1>(),
            encrypted: false,
        }
    }
    pub fn encrypted_response(encrypted: [u8; 5]) -> Self {
        let mut bytes = [0; 13];
        for (slot, byte) in bytes.iter_mut().zip(encrypted) {
            *slot = byte;
        }
        Self {
            bytes,
            len: PrefixLength::constant::<5>(),
            encrypted: true,
        }
    }
    pub const fn missing_key_reject() -> Self {
        Self {
            bytes: [0x0d, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            len: PrefixLength::constant::<2>(),
            encrypted: false,
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use super::*;
    #[test]
    fn encryption_request_retains_wire_order_and_refuses_every_truncation() {
        let bytes = [
            3, 1, 2, 3, 4, 5, 6, 7, 8, 0x34, 0x12, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
        ];
        let request = LeEncryptionRequest::decode(&bytes).unwrap();
        assert_eq!(request.key.random_number(), [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(request.key.encrypted_diversifier(), 0x1234);
        assert_eq!(request.diversifier, [11, 12, 13, 14, 15, 16, 17, 18]);
        assert_eq!(request.initialization_vector, [19, 20, 21, 22]);
        for length in 0..bytes.len() {
            assert!(LeEncryptionRequest::decode(&bytes[..length]).is_none());
        }
        let mut wrong = bytes;
        wrong[0] = 4;
        assert!(LeEncryptionRequest::decode(&wrong).is_none());
        assert!(LeEncryptionRequest::decode(&[3; 24]).is_none());
    }
    #[test]
    fn encryption_control_responses_preserve_extent_and_encryption_metadata() {
        let response = LeEncryptionControlResponse::encryption_response(
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            [17, 18, 19, 20, 21, 22, 23, 24],
        );
        assert_eq!(
            response.as_bytes(),
            [4, 9, 10, 11, 12, 13, 14, 15, 16, 21, 22, 23, 24]
        );
        assert!(!response.is_encrypted());
        assert_eq!(LeEncryptionControlResponse::start_request().as_bytes(), [5]);
        assert_eq!(
            LeEncryptionControlResponse::missing_key_reject().as_bytes(),
            [0x0d, 6]
        );
        let response = LeEncryptionControlResponse::encrypted_response([7, 8, 9, 10, 11]);
        assert_eq!(response.as_bytes(), [7, 8, 9, 10, 11]);
        assert!(response.is_encrypted());
    }
}
