//! A length is an enum variant, and the accessible prefix is an array of
//! that variant's exact length. Every variant also owns the entire inactive
//! tail, so wire SSID bytes survive and erasure reaches all initialized storage.
use super::NetworkCredentialsError;
use zeroize::Zeroize;

macro_rules! bounded_bytes {
    ($name:ident, $capacity:expr, $error:ident, [$($variant:ident = $length:literal),* $(,)?]) => {
        #[repr(u8)]
        pub(super) enum $name {
            $($variant {
                prefix: [u8; $length],
                tail: [u8; $capacity - $length],
            },)*
        }

        impl $name {
            pub(super) fn from_slice(bytes: &[u8]) -> Result<Self, NetworkCredentialsError> {
                match bytes.len() {
                    $($length => Ok(Self::$variant {
                        prefix: bytes.try_into().map_err(|_| NetworkCredentialsError::$error)?,
                        tail: [0; $capacity - $length],
                    }),)*
                    _ => Err(NetworkCredentialsError::$error),
                }
            }

            pub(super) fn from_full(
                bytes: &[u8; $capacity], length: usize,
            ) -> Result<Self, NetworkCredentialsError> {
                match length {
                    $($length => {
                        let (prefix, tail) = bytes.split_first_chunk::<$length>()
                            .ok_or(NetworkCredentialsError::$error)?;
                        Ok(Self::$variant {
                            prefix: *prefix,
                            tail: tail.try_into().map_err(|_| NetworkCredentialsError::$error)?,
                        })
                    },)*
                    _ => Err(NetworkCredentialsError::$error),
                }
            }

            pub(super) fn parts(&self) -> (&[u8], &[u8]) {
                match self {
                    $(Self::$variant { prefix, tail } => (prefix.as_slice(), tail.as_slice()),)*
                }
            }

            pub(super) fn as_slice(&self) -> &[u8] { self.parts().0 }
        }

        impl Zeroize for $name {
            fn zeroize(&mut self) {
                match self {
                    $(Self::$variant { prefix, tail } => { prefix.zeroize(); tail.zeroize(); },)*
                }
                // The old value has been erased before changing its length.
                *self = Self::B0 { prefix: [], tail: [0; $capacity] };
            }
        }
    };
}

bounded_bytes!(
    Ssid,
    super::MAX_SSID_BYTES,
    SsidLength,
    [
        B0 = 0,
        B1 = 1,
        B2 = 2,
        B3 = 3,
        B4 = 4,
        B5 = 5,
        B6 = 6,
        B7 = 7,
        B8 = 8,
        B9 = 9,
        B10 = 10,
        B11 = 11,
        B12 = 12,
        B13 = 13,
        B14 = 14,
        B15 = 15,
        B16 = 16,
        B17 = 17,
        B18 = 18,
        B19 = 19,
        B20 = 20,
        B21 = 21,
        B22 = 22,
        B23 = 23,
        B24 = 24,
        B25 = 25,
        B26 = 26,
        B27 = 27,
        B28 = 28,
        B29 = 29,
        B30 = 30,
        B31 = 31,
        B32 = 32,
    ]
);

bounded_bytes!(
    Secret,
    super::MAX_PASSPHRASE_BYTES,
    PassphraseLength,
    [
        B0 = 0,
        B1 = 1,
        B2 = 2,
        B3 = 3,
        B4 = 4,
        B5 = 5,
        B6 = 6,
        B7 = 7,
        B8 = 8,
        B9 = 9,
        B10 = 10,
        B11 = 11,
        B12 = 12,
        B13 = 13,
        B14 = 14,
        B15 = 15,
        B16 = 16,
        B17 = 17,
        B18 = 18,
        B19 = 19,
        B20 = 20,
        B21 = 21,
        B22 = 22,
        B23 = 23,
        B24 = 24,
        B25 = 25,
        B26 = 26,
        B27 = 27,
        B28 = 28,
        B29 = 29,
        B30 = 30,
        B31 = 31,
        B32 = 32,
        B33 = 33,
        B34 = 34,
        B35 = 35,
        B36 = 36,
        B37 = 37,
        B38 = 38,
        B39 = 39,
        B40 = 40,
        B41 = 41,
        B42 = 42,
        B43 = 43,
        B44 = 44,
        B45 = 45,
        B46 = 46,
        B47 = 47,
        B48 = 48,
        B49 = 49,
        B50 = 50,
        B51 = 51,
        B52 = 52,
        B53 = 53,
        B54 = 54,
        B55 = 55,
        B56 = 56,
        B57 = 57,
        B58 = 58,
        B59 = 59,
        B60 = 60,
        B61 = 61,
        B62 = 62,
        B63 = 63,
    ]
);
