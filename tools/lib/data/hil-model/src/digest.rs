//! SHA-256 digests in their one canonical text form, and the canonical
//! JSON that instance identities hash.

use core::fmt;
use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use sha2::{Digest, Sha256 as Hasher};

/// A SHA-256 digest, written as exactly 64 lowercase hexadecimal digits;
/// anything else is refused when read.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Sha256(pub [u8; 32]);

impl Sha256 {
    pub fn of(bytes: &[u8]) -> Self {
        Self(Hasher::digest(bytes).into())
    }

    /// The canonical form only.
    pub fn parse(text: &str) -> Result<Self, InvalidDigest> {
        let digits = text.as_bytes();
        if digits.len() != 64 {
            return Err(InvalidDigest);
        }
        let value = |digit: u8| match digit {
            b'0'..=b'9' => Ok(digit - b'0'),
            b'a'..=b'f' => Ok(digit - b'a' + 10),
            _ => Err(InvalidDigest),
        };
        let mut bytes = [0; 32];
        for (byte, pair) in bytes.iter_mut().zip(digits.chunks_exact(2)) {
            *byte = (value(pair[0])? << 4) | value(pair[1])?;
        }
        Ok(Self(bytes))
    }
}

/// A digest that is not 64 lowercase hexadecimal digits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidDigest;

impl fmt::Display for InvalidDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a SHA-256 digest is 64 lowercase hexadecimal digits")
    }
}

impl std::error::Error for InvalidDigest {}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl Serialize for Sha256 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Sha256 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(D::Error::custom)
    }
}

/// The canonical bytes of a JSON value: objects with their keys sorted by
/// byte order, no whitespace, numbers and strings as `serde_json` writes
/// them. It does not depend on the order in which a serializer emits map
/// entries or on `serde_json`'s map features.
pub fn canonical_json(value: &serde_json::Value) -> Vec<u8> {
    fn write(value: &serde_json::Value, out: &mut Vec<u8>) {
        match value {
            serde_json::Value::Object(map) => {
                let sorted: BTreeMap<&str, &serde_json::Value> = map
                    .iter()
                    .map(|(key, value)| (key.as_str(), value))
                    .collect();
                out.push(b'{');
                for (index, (key, value)) in sorted.into_iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    out.extend(serde_json::Value::from(key).to_string().into_bytes());
                    out.push(b':');
                    write(value, out);
                }
                out.push(b'}');
            }
            serde_json::Value::Array(items) => {
                out.push(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    write(item, out);
                }
                out.push(b']');
            }
            // Negative zero equals zero; both are written as `0.0`.
            serde_json::Value::Number(number)
                if number.as_f64() == Some(0.0) && number.is_f64() =>
            {
                out.extend(b"0.0");
            }
            scalar => out.extend(scalar.to_string().into_bytes()),
        }
    }
    let mut out = Vec::new();
    write(value, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_canonical_digest_form_reads_back() {
        let digest = Sha256([0xa5; 32]);
        let json = serde_json::to_string(&digest).unwrap();
        assert_eq!(serde_json::from_str::<Sha256>(&json).unwrap(), digest);
        for text in [
            format!("\"€{}\"", "a".repeat(61)),
            format!("\"+f{}\"", "0".repeat(62)),
            format!("\"A{}\"", "0".repeat(63)),
            format!("\"{}\"", "0".repeat(63)),
        ] {
            assert!(serde_json::from_str::<Sha256>(&text).is_err(), "{text}");
        }
    }

    #[test]
    fn negative_zero_is_written_as_zero() {
        let negative = serde_json::json!({ "x": -0.0_f64 });
        let positive = serde_json::json!({ "x": 0.0_f64 });
        assert_eq!(canonical_json(&negative), canonical_json(&positive));
        assert_eq!(canonical_json(&negative), br#"{"x":0.0}"#.to_vec());
    }

    #[test]
    fn canonical_json_does_not_depend_on_key_order() {
        let a: serde_json::Value =
            serde_json::from_str(r#"{"b":1,"a":{"d":[1,{"z":0,"y":"x"}],"c":null}}"#).unwrap();
        let b: serde_json::Value =
            serde_json::from_str(r#"{"a":{"c":null,"d":[1,{"y":"x","z":0}]},"b":1}"#).unwrap();
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert_eq!(
            canonical_json(&a),
            br#"{"a":{"c":null,"d":[1,{"y":"x","z":0}]},"b":1}"#.to_vec()
        );
    }
}
