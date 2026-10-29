//! Public test material only: one host-requested operation on the product
//! AES-128 RFC 3394 engine. No vectors or test loop execute autonomously.
use heapless::Vec;
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

pub const AES_KEY_DATA_CAPACITY: usize = 512;
pub type AesKeyData = Vec<u8, AES_KEY_DATA_CAPACITY>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct AesKeyUnwrap {
    pub operation: u32,
    pub kek: [u8; 16],
    pub wrapped: AesKeyData,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum AesUnwrapError {
    InvalidLength,
    IntegrityCheckFailed,
    Unavailable,
    Timeout { observation: u32 },
    InvalidState { observation: u32 },
    TimeOverflow,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct AesKeyUnwrapped {
    pub operation: u32,
    pub completed: u32,
    pub succeeded: u32,
    pub refused: u32,
    pub result: Result<AesKeyData, AesUnwrapError>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn largest_product_request_and_response_fit_the_existing_frame() {
        let data = AesKeyData::from_slice(&[0xff; AES_KEY_DATA_CAPACITY]).unwrap();
        let request = AesKeyUnwrap {
            operation: u32::MAX,
            kek: [0xff; 16],
            wrapped: data.clone(),
        };
        let response = AesKeyUnwrapped {
            operation: u32::MAX,
            completed: u32::MAX,
            succeeded: u32::MAX,
            refused: u32::MAX,
            result: Ok(data),
        };
        let mut bytes = [0; crate::MAX_POSTCARD_BYTES];
        for len in [
            postcard::to_slice(&request, &mut bytes).unwrap().len(),
            postcard::to_slice(&response, &mut bytes).unwrap().len(),
        ] {
            assert!(len <= crate::MAX_POSTCARD_BYTES);
        }
    }
}
