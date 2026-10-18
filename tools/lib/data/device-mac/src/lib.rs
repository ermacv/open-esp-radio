//! A board's identity, [`DeviceId`]: its MAC in the one canonical form
//! (upper case, colon separated, six bytes), as Espressif USB Serial/JTAG
//! ports report it as their USB serial number. Every devices-layer API keys
//! a board by a `DeviceId`, and [`DeviceId::parse`] is the only way to make
//! one, so two spellings of a MAC never name two boards.
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// A board's canonical MAC: `02:00:00:00:25:64`.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct DeviceId(String);

impl DeviceId {
    /// The normaliser: `text` with or without `:` or `-` separators, in any
    /// case, as the board's id.
    pub fn parse(text: &str) -> Result<Self, String> {
        let digits = text
            .chars()
            .filter(|character| !matches!(character, ':' | '-'))
            .collect::<String>();
        if digits.len() != 12 || !digits.chars().all(|digit| digit.is_ascii_hexdigit()) {
            return Err(format!("`{text}` is not a six-byte MAC address"));
        }
        Ok(Self(
            digits
                .to_ascii_uppercase()
                .as_bytes()
                .chunks(2)
                .map(|pair| String::from_utf8_lossy(pair).into_owned())
                .collect::<Vec<_>>()
                .join(":"),
        ))
    }

    /// The canonical form.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The MAC without separators, for file names: `020000002564`.
    pub fn compact(&self) -> String {
        self.0.replace(':', "")
    }
}

impl std::ops::Deref for DeviceId {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for DeviceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.pad(&self.0)
    }
}

impl std::str::FromStr for DeviceId {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        Self::parse(text)
    }
}

impl TryFrom<String> for DeviceId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        Self::parse(&text)
    }
}

impl From<DeviceId> for String {
    fn from(id: DeviceId) -> Self {
        id.0
    }
}

impl PartialEq<str> for DeviceId {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for DeviceId {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<String> for DeviceId {
    fn eq(&self, other: &String) -> bool {
        self.0 == *other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macs_are_normalized_and_invalid_ones_refused() {
        let id = DeviceId::parse("02:00:00:00:25:64").unwrap();
        assert_eq!(id.as_str(), "02:00:00:00:25:64");
        assert_eq!(DeviceId::parse("020000002564").unwrap(), id);
        assert_eq!(DeviceId::parse("02-00-00-00-25-64").unwrap(), id);
        assert_eq!(id.compact(), "020000002564");
        for invalid in ["", "38:44", "zz:44:be:aa:25:64", "/dev/ttyACM0"] {
            assert!(DeviceId::parse(invalid).is_err(), "{invalid}");
        }
        assert!(serde_json::from_str::<DeviceId>("\"38:44\"").is_err());
        assert_eq!(
            serde_json::from_str::<DeviceId>("\"020000002564\"").unwrap(),
            id
        );
    }
}
