use core::fmt;

use postcard_schema::Schema;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeStruct};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

mod storage;
use storage::{Secret, Ssid};

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
/// Lengths are private array variants; callers cannot invalidate the bounds.
///
/// ```compile_fail
/// use oer_hil_protocol::wifi::NetworkCredentials;
/// let mut credentials = NetworkCredentials::try_new(b"ssid", b"password").unwrap();
/// credentials.ssid_length = 255;
/// ```
pub struct NetworkCredentials {
    ssid: Ssid,
    passphrase: Passphrase,
}

impl NetworkCredentials {
    pub fn try_new(ssid: &[u8], passphrase: &[u8]) -> Result<Self, NetworkCredentialsError> {
        let ssid = Ssid::from_slice(ssid)?;
        if ssid.as_slice().is_empty() {
            return Err(NetworkCredentialsError::SsidLength);
        }
        if passphrase.len() < MIN_PASSPHRASE_BYTES {
            return Err(NetworkCredentialsError::PassphraseLength);
        }
        let passphrase = Passphrase(Secret::from_slice(passphrase)?);
        Ok(Self { ssid, passphrase })
    }

    pub fn ssid(&self) -> &[u8] {
        self.ssid.as_slice()
    }

    pub fn passphrase(&self) -> &[u8] {
        self.passphrase.0.as_slice()
    }

    pub fn validate(&self) -> Result<(), NetworkCredentialsError> {
        if !(1..=MAX_SSID_BYTES).contains(&self.ssid().len()) {
            return Err(NetworkCredentialsError::SsidLength);
        }
        if !(MIN_PASSPHRASE_BYTES..=MAX_PASSPHRASE_BYTES).contains(&self.passphrase().len()) {
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
        let mut wire_ssid = [0; MAX_SSID_BYTES];
        let (prefix, tail) = self.ssid.parts();
        for (slot, byte) in wire_ssid.iter_mut().zip(prefix.iter().chain(tail)) {
            *slot = *byte;
        }
        value.serialize_field("ssid", &wire_ssid)?;
        value.serialize_field("ssid_length", &(self.ssid().len() as u8))?;
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
        let wire_ssid = Zeroizing::new(wire.ssid);
        let value = Self {
            ssid: Ssid::from_full(&wire_ssid, usize::from(wire.ssid_length))
                .map_err(de::Error::custom)?,
            passphrase: wire.passphrase,
        };
        value.validate().map_err(de::Error::custom)?;
        Ok(value)
    }
}

// The schema describes the accepted wire, independently of the bounded owner.
impl Schema for NetworkCredentials {
    const SCHEMA: &'static postcard_schema::schema::NamedType = {
        use postcard_schema::schema::{DataModelType, NamedType, NamedValue};
        &NamedType {
            name: "NetworkCredentials",
            ty: &DataModelType::Struct(&[
                &NamedValue {
                    name: "ssid",
                    ty: <[u8; MAX_SSID_BYTES]>::SCHEMA,
                },
                &NamedValue {
                    name: "ssid_length",
                    ty: u8::SCHEMA,
                },
                &NamedValue {
                    name: "passphrase",
                    ty: Passphrase::SCHEMA,
                },
            ]),
        }
    };
}

// Drop also erases a successfully decoded passphrase if the following SSID
// validation fails. The decoder's partial buffer has its own Zeroizing guard.
struct Passphrase(Secret);

impl Schema for Passphrase {
    const SCHEMA: &'static postcard_schema::schema::NamedType =
        <heapless::Vec<u8, MAX_PASSPHRASE_BYTES> as Schema>::SCHEMA;
}

impl Zeroize for Passphrase {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl Drop for Passphrase {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl Serialize for Passphrase {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.as_slice().serialize(serializer)
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
                let mut bytes = Zeroizing::new([0; MAX_PASSPHRASE_BYTES]);
                let mut length = 0_usize;
                while let Some(byte) = sequence.next_element::<u8>()? {
                    let Some(slot) = bytes.get_mut(length) else {
                        return Err(de::Error::custom(NetworkCredentialsError::PassphraseLength));
                    };
                    *slot = byte;
                    length = length.checked_add(1).ok_or_else(|| {
                        de::Error::custom(NetworkCredentialsError::PassphraseLength)
                    })?;
                }
                if length < MIN_PASSPHRASE_BYTES {
                    return Err(de::Error::custom(NetworkCredentialsError::PassphraseLength));
                }
                Secret::from_full(&bytes, length)
                    .map(Passphrase)
                    .map_err(de::Error::custom)
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi::{StartStation, StationPowerSave, StationStart};
    use crate::{EncodeError, Envelope, FrameEncoder};

    // The accepted wire before bounded in-memory storage: every fixed SSID
    // byte, its logical length, then a bounded passphrase sequence.
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct WireCredentials {
        ssid: [u8; MAX_SSID_BYTES],
        ssid_length: u8,
        passphrase: heapless::Vec<u8, MAX_PASSPHRASE_BYTES>,
    }

    fn assert_zero_storage(storage: (&[u8], &[u8]), capacity: usize) {
        assert_eq!(storage.0.len() + storage.1.len(), capacity);
        assert!(storage.0.iter().chain(storage.1).all(|byte| *byte == 0));
    }

    #[test]
    fn every_length_keeps_all_wire_bytes_and_has_infallible_access() {
        for ssid_length in 1..=MAX_SSID_BYTES {
            for passphrase_length in MIN_PASSPHRASE_BYTES..=MAX_PASSPHRASE_BYTES {
                let original = WireCredentials {
                    ssid: core::array::from_fn(|index| {
                        (index as u8).wrapping_mul(7).wrapping_add(3)
                    }),
                    ssid_length: ssid_length as u8,
                    passphrase: heapless::Vec::from_slice(
                        &[0xa5; MAX_PASSPHRASE_BYTES][..passphrase_length],
                    )
                    .unwrap(),
                };
                let mut before = [0; 128];
                let before = postcard::to_slice(&original, &mut before).unwrap();
                let credentials: NetworkCredentials = postcard::from_bytes(before).unwrap();
                assert_eq!(credentials.ssid(), &original.ssid[..ssid_length]);
                assert_eq!(credentials.passphrase(), original.passphrase.as_slice());
                let mut after = [0; 128];
                let after = postcard::to_slice(&credentials, &mut after).unwrap();
                assert_eq!(after, before);
                assert_eq!(
                    postcard::from_bytes::<WireCredentials>(after).unwrap(),
                    original
                );
            }
        }
    }

    #[test]
    fn erase_zeroes_every_byte_of_every_storage_variant_and_resets_lengths() {
        for length in 0..=MAX_SSID_BYTES {
            let mut ssid = Ssid::from_full(&[0xa5; MAX_SSID_BYTES], length).unwrap();
            assert!(
                ssid.parts()
                    .0
                    .iter()
                    .chain(ssid.parts().1)
                    .all(|byte| *byte == 0xa5)
            );
            ssid.zeroize();
            assert!(ssid.as_slice().is_empty());
            assert_zero_storage(ssid.parts(), MAX_SSID_BYTES);
        }
        for length in 0..=MAX_PASSPHRASE_BYTES {
            let mut passphrase =
                Passphrase(Secret::from_full(&[0xa5; MAX_PASSPHRASE_BYTES], length).unwrap());
            assert!(
                passphrase
                    .0
                    .parts()
                    .0
                    .iter()
                    .chain(passphrase.0.parts().1)
                    .all(|byte| *byte == 0xa5)
            );
            passphrase.zeroize();
            assert!(passphrase.0.as_slice().is_empty());
            assert_zero_storage(passphrase.0.parts(), MAX_PASSPHRASE_BYTES);
        }
        for ssid_length in 1..=MAX_SSID_BYTES {
            let mut credentials = NetworkCredentials {
                ssid: Ssid::from_full(&[0xa5; MAX_SSID_BYTES], ssid_length).unwrap(),
                passphrase: Passphrase(
                    Secret::from_full(&[0x5a; MAX_PASSPHRASE_BYTES], 8).unwrap(),
                ),
            };
            credentials.clear_passphrase();
            assert_zero_storage(credentials.passphrase.0.parts(), MAX_PASSPHRASE_BYTES);
            assert_eq!(credentials.ssid(), &[0xa5; MAX_SSID_BYTES][..ssid_length]);
            assert!(credentials.ssid.parts().1.iter().all(|byte| *byte == 0xa5));
            credentials.zeroize();
            assert!(credentials.ssid().is_empty());
            assert_zero_storage(credentials.ssid.parts(), MAX_SSID_BYTES);
            assert_zero_storage(credentials.passphrase.0.parts(), MAX_PASSPHRASE_BYTES);
        }
    }

    #[test]
    fn invalid_lengths_are_rejected_without_replacing_bytes() {
        assert_eq!(
            Ssid::from_full(&[0xa5; MAX_SSID_BYTES], MAX_SSID_BYTES + 1).err(),
            Some(NetworkCredentialsError::SsidLength)
        );
        assert_eq!(
            Secret::from_full(&[0xa5; MAX_PASSPHRASE_BYTES], MAX_PASSPHRASE_BYTES + 1).err(),
            Some(NetworkCredentialsError::PassphraseLength)
        );
        assert_eq!(
            NetworkCredentials::try_new(&[0; 33], b"short").err(),
            Some(NetworkCredentialsError::SsidLength)
        );
    }

    #[test]
    fn clear_passphrase_erases_storage_and_prevents_serializing_a_station_command() {
        for length in [MIN_PASSPHRASE_BYTES, MAX_PASSPHRASE_BYTES] {
            let mut command = Envelope::new(
                7,
                11,
                13,
                17,
                StartStation(StationStart {
                    credentials: NetworkCredentials::try_new(
                        b"test-network",
                        &[0xa5; 63][..length],
                    )
                    .unwrap(),
                    power_save: StationPowerSave::Off,
                }),
            );
            assert!(FrameEncoder::new().encode(&command).is_ok());

            command.body.0.credentials.clear_passphrase();

            let credentials = &command.body.0.credentials;
            assert!(
                credentials
                    .passphrase
                    .0
                    .parts()
                    .0
                    .iter()
                    .chain(credentials.passphrase.0.parts().1)
                    .all(|byte| *byte == 0)
            );
            assert!(credentials.passphrase().is_empty());
            assert_eq!(
                credentials.validate(),
                Err(NetworkCredentialsError::PassphraseLength)
            );
            assert_eq!(
                FrameEncoder::new().encode(&command),
                Err(EncodeError::Serialize)
            );
        }
    }

    #[test]
    fn clear_erases_the_entire_passphrase_storage_and_preserves_ssid() {
        let mut credentials = NetworkCredentials::try_new(b"test-network", &[0xa5; 63]).unwrap();
        credentials.clear_passphrase();
        assert!(
            credentials
                .passphrase
                .0
                .parts()
                .0
                .iter()
                .chain(credentials.passphrase.0.parts().1)
                .all(|byte| *byte == 0)
        );
        assert!(credentials.passphrase().is_empty());
        assert_eq!(credentials.ssid(), b"test-network");
        assert_eq!(
            credentials.validate(),
            Err(NetworkCredentialsError::PassphraseLength)
        );
        assert!(postcard::to_slice(&credentials, &mut [0; 128]).is_err());
        credentials.zeroize();
        assert!(
            credentials
                .ssid
                .parts()
                .0
                .iter()
                .chain(credentials.ssid.parts().1)
                .all(|byte| *byte == 0)
        );
        assert!(credentials.ssid().is_empty());
        fn requires_erasure_on_drop<T: ZeroizeOnDrop>() {}
        requires_erasure_on_drop::<NetworkCredentials>();
    }
}
