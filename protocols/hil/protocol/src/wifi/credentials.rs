use core::fmt;

use postcard_schema::Schema;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeStruct};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const MAX_SSID_BYTES: usize = 32;
pub const MIN_PASSPHRASE_BYTES: usize = 8;
pub const MAX_PASSPHRASE_BYTES: usize = 63;

/// No input bytes are retained in a validation error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkCredentialsError {
    SsidLength,
    PassphraseLength,
}

impl fmt::Display for NetworkCredentialsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::SsidLength => "SSID length must be 1..=32 bytes",
            Self::PassphraseLength => "passphrase length must be 8..=63 bytes",
        })
    }
}

impl core::error::Error for NetworkCredentialsError {}

/// Owned run-time credentials. Only binary wire serialization exposes bytes.
///
/// This owner is neither `Clone` nor `Copy`. Debug and human-readable
/// serializers redact the entire value; human-readable decoding is rejected.
#[derive(Schema)]
pub struct NetworkCredentials {
    ssid: [u8; MAX_SSID_BYTES],
    ssid_length: u8,
    passphrase: Passphrase,
}

impl NetworkCredentials {
    pub fn try_new(ssid: &[u8], passphrase: &[u8]) -> Result<Self, NetworkCredentialsError> {
        if !(1..=MAX_SSID_BYTES).contains(&ssid.len()) {
            return Err(NetworkCredentialsError::SsidLength);
        }
        if !(MIN_PASSPHRASE_BYTES..=MAX_PASSPHRASE_BYTES).contains(&passphrase.len()) {
            return Err(NetworkCredentialsError::PassphraseLength);
        }
        let mut value = Self {
            ssid: [0; MAX_SSID_BYTES],
            ssid_length: u8::try_from(ssid.len())
                .map_err(|_| NetworkCredentialsError::SsidLength)?,
            passphrase: Passphrase::empty(),
        };
        value.ssid[..ssid.len()].copy_from_slice(ssid);
        value.passphrase.bytes[..passphrase.len()].copy_from_slice(passphrase);
        value.passphrase.length = passphrase.len();
        Ok(value)
    }

    pub fn ssid(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_length)]
    }

    pub fn passphrase(&self) -> &[u8] {
        &self.passphrase.bytes[..self.passphrase.length]
    }

    pub fn validate(&self) -> Result<(), NetworkCredentialsError> {
        if !(1..=MAX_SSID_BYTES).contains(&usize::from(self.ssid_length)) {
            return Err(NetworkCredentialsError::SsidLength);
        }
        if !(MIN_PASSPHRASE_BYTES..=MAX_PASSPHRASE_BYTES).contains(&self.passphrase.length) {
            return Err(NetworkCredentialsError::PassphraseLength);
        }
        Ok(())
    }

    /// Erase the full backing storage after PMK derivation. A cleared owner
    /// retains its SSID but cannot be sent as a new station command.
    pub fn clear_passphrase(&mut self) {
        self.passphrase.zeroize();
    }
}

impl fmt::Debug for NetworkCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NetworkCredentials(<redacted>)")
    }
}

impl Zeroize for NetworkCredentials {
    fn zeroize(&mut self) {
        self.ssid.zeroize();
        self.ssid_length.zeroize();
        self.passphrase.zeroize();
    }
}

impl Drop for NetworkCredentials {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl ZeroizeOnDrop for NetworkCredentials {}

impl Serialize for NetworkCredentials {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            return serializer.serialize_str("<redacted>");
        }
        self.validate().map_err(serde::ser::Error::custom)?;
        let mut value = serializer.serialize_struct("NetworkCredentials", 3)?;
        value.serialize_field("ssid", &self.ssid)?;
        value.serialize_field("ssid_length", &self.ssid_length)?;
        value.serialize_field("passphrase", &self.passphrase)?;
        value.end()
    }
}

impl<'de> Deserialize<'de> for NetworkCredentials {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            return Err(de::Error::custom("credentials require binary HIL decoding"));
        }
        #[derive(Deserialize)]
        struct Wire {
            ssid: [u8; MAX_SSID_BYTES],
            ssid_length: u8,
            passphrase: Passphrase,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            ssid: wire.ssid,
            ssid_length: wire.ssid_length,
            passphrase: wire.passphrase,
        };
        value.validate().map_err(de::Error::custom)?;
        Ok(value)
    }
}

// A guard also erases a partially decoded passphrase on every error path.
// Its wire schema is a bounded byte sequence, independent of storage layout.
struct Passphrase {
    bytes: [u8; MAX_PASSPHRASE_BYTES],
    length: usize,
}

impl Passphrase {
    const fn empty() -> Self {
        Self {
            bytes: [0; MAX_PASSPHRASE_BYTES],
            length: 0,
        }
    }
}

impl Schema for Passphrase {
    const SCHEMA: &'static postcard_schema::schema::NamedType =
        <heapless::Vec<u8, MAX_PASSPHRASE_BYTES> as Schema>::SCHEMA;
}

impl Zeroize for Passphrase {
    fn zeroize(&mut self) {
        self.bytes.zeroize();
        self.length.zeroize();
    }
}

impl Drop for Passphrase {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl Serialize for Passphrase {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.bytes[..self.length].serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Passphrase {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = Passphrase;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded passphrase byte sequence")
            }

            fn visit_seq<A: de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut value = Passphrase::empty();
                while let Some(byte) = sequence.next_element::<u8>()? {
                    let Some(slot) = value.bytes.get_mut(value.length) else {
                        return Err(de::Error::custom(NetworkCredentialsError::PassphraseLength));
                    };
                    *slot = byte;
                    value.length += 1;
                }
                if value.length < MIN_PASSPHRASE_BYTES {
                    return Err(de::Error::custom(NetworkCredentialsError::PassphraseLength));
                }
                Ok(value)
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_erases_the_entire_passphrase_storage_and_preserves_ssid() {
        let mut credentials = NetworkCredentials::try_new(b"test-network", &[0xa5; 63]).unwrap();
        credentials.clear_passphrase();
        assert_eq!(credentials.passphrase.bytes, [0; MAX_PASSPHRASE_BYTES]);
        assert_eq!(credentials.passphrase.length, 0);
        assert_eq!(credentials.ssid(), b"test-network");
        assert_eq!(
            credentials.validate(),
            Err(NetworkCredentialsError::PassphraseLength)
        );
        assert!(postcard::to_slice(&credentials, &mut [0; 128]).is_err());
        credentials.zeroize();
        assert_eq!(credentials.ssid, [0; MAX_SSID_BYTES]);
        assert_eq!(credentials.ssid_length, 0);
        fn requires_erasure_on_drop<T: ZeroizeOnDrop>() {}
        requires_erasure_on_drop::<NetworkCredentials>();
    }
}
