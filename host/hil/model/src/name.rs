//! Validated names: kebab-case scenario, instance, chip, fixture and
//! composition names, the dotted names of spans, measurements, operations
//! and capabilities, and the contract path.

use core::fmt;
use std::borrow::Cow;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

/// A name's text: borrowed for compile-time names, owned for parsed ones.
type Text = Cow<'static, str>;

/// The grammars a name can follow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Grammar {
    /// Lowercase ASCII letters, digits and single hyphens, starting with a
    /// letter and not ending with a hyphen, at most 64 bytes:
    /// `lower-mac-tx-ownership`.
    Kebab,
    /// Kebab segments, each at most 64 bytes, joined by single dots, at
    /// most 96 bytes in all: `lower-mac.drain`, `boot-to-hello`.
    Dotted,
    /// A package name (lowercase letters, digits, hyphens and
    /// underscores, starting with a letter) followed by one or more `::`
    /// and segments of ASCII letters, digits and underscores that start
    /// with a letter or an underscore, at most 128 bytes:
    /// `oer-ieee80211-lower-mac::LowerMacPort`. Keywords are not refused:
    /// the name identifies a contract, it is not compiled.
    Path,
}

const fn identifier(bytes: &[u8], start: usize, end: usize) -> bool {
    if start >= end || !(bytes[start].is_ascii_alphabetic() || bytes[start] == b'_') {
        return false;
    }
    let mut index = start;
    while index < end {
        if !(bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_') {
            return false;
        }
        index += 1;
    }
    true
}

const fn package(bytes: &[u8], end: usize) -> bool {
    if end == 0 || !bytes[0].is_ascii_lowercase() {
        return false;
    }
    let mut index = 0;
    while index < end {
        let byte = bytes[index];
        if !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_') {
            return false;
        }
        index += 1;
    }
    true
}

const fn kebab(bytes: &[u8], start: usize, end: usize) -> bool {
    if start >= end || !bytes[start].is_ascii_lowercase() || bytes[end - 1] == b'-' {
        return false;
    }
    let mut index = start;
    while index < end {
        let byte = bytes[index];
        let allowed = byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
        if !allowed || (byte == b'-' && index > start && bytes[index - 1] == b'-') {
            return false;
        }
        index += 1;
    }
    true
}

impl Grammar {
    pub const fn accepts(self, name: &str) -> bool {
        let bytes = name.as_bytes();
        match self {
            Self::Kebab => bytes.len() <= 64 && kebab(bytes, 0, bytes.len()),
            Self::Dotted => {
                if bytes.len() > 96 {
                    return false;
                }
                let mut start = 0;
                let mut index = 0;
                while index <= bytes.len() {
                    if index == bytes.len() || bytes[index] == b'.' {
                        if index - start > 64 || !kebab(bytes, start, index) {
                            return false;
                        }
                        start = index + 1;
                    }
                    index += 1;
                }
                true
            }
            Self::Path => {
                if bytes.len() > 128 {
                    return false;
                }
                // The package, then `::`-separated identifiers.
                let mut index = 0;
                while index + 1 < bytes.len() && !(bytes[index] == b':' && bytes[index + 1] == b':')
                {
                    index += 1;
                }
                if index + 1 >= bytes.len() || !package(bytes, index) {
                    return false;
                }
                let mut start = index + 2;
                let mut cursor = start;
                while cursor <= bytes.len() {
                    if cursor == bytes.len()
                        || (bytes[cursor] == b':'
                            && cursor + 1 < bytes.len()
                            && bytes[cursor + 1] == b':')
                    {
                        if !identifier(bytes, start, cursor) {
                            return false;
                        }
                        start = cursor + 2;
                        cursor = start;
                        continue;
                    }
                    cursor += 1;
                }
                true
            }
        }
    }
}

/// A name that does not follow its grammar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidName {
    pub name: String,
    pub grammar: Grammar,
}

impl fmt::Display for InvalidName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not a {:?} name", self.name, self.grammar)
    }
}

impl std::error::Error for InvalidName {}

macro_rules! names {
    ($($(#[$doc:meta])* $name:ident: $grammar:ident;)*) => {$(
        $(#[$doc])*
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(Text);

        impl $name {
            /// A name known at compile time, or `None` when it breaks its
            /// grammar. [`name!`](crate::name) turns `None` into a
            /// compile error.
            pub const fn checked(name: &'static str) -> Option<Self> {
                if Grammar::$grammar.accepts(name) {
                    Some(Self(Text::Borrowed(name)))
                } else {
                    None
                }
            }

            /// A name read at run time: a requirement, a record, a command
            /// line.
            pub fn parse(name: &str) -> Result<Self, InvalidName> {
                if Grammar::$grammar.accepts(name) {
                    Ok(Self(Text::Owned(name.to_owned())))
                } else {
                    Err(InvalidName { name: name.to_owned(), grammar: Grammar::$grammar })
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                Self::parse(&text).map_err(D::Error::custom)
            }
        }
    )*};
}

names! {
    /// The stable identity of a scenario type.
    ScenarioId: Kebab;
    /// One parameterised instance of a scenario.
    InstanceName: Kebab;
    /// A span measured on the stand, such as `boot-to-hello`.
    SpanName: Dotted;
    /// A measurement a run records, such as `connect.attempts`.
    MeasurementName: Dotted;
    /// An infrastructure operation, such as `link.sync`.
    Operation: Dotted;
    /// A chip of `platform/<chip>/chip.toml`, such as `esp32s31`.
    ChipId: Kebab;
    /// A fixture of the stand file, such as `hostapd`.
    FixtureName: Kebab;
    /// A product composition crate, such as `oer-esp32s31-ieee80211-system`.
    CompositionName: Kebab;
    /// The contract a scenario exercises, by its path:
    /// `oer-ieee80211-lower-mac::LowerMacPort`.
    ContractName: Path;
    /// A capability the HIL agent serves, by its `hil/protocol` module and
    /// name, such as `conformance.lower-mac`; the image keys derive from
    /// these through `oer-hil-image-keys`.
    CapabilityKey: Dotted;
}

/// One instance of one scenario: `scenario/instance`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ScenarioInstance {
    pub scenario: ScenarioId,
    pub instance: InstanceName,
}

impl fmt::Display for ScenarioInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.scenario, self.instance)
    }
}

/// A name checked at compile time: `name!(ScenarioId, "lower-mac-tx")`.
/// A name that breaks its grammar does not compile.
#[macro_export]
macro_rules! name {
    ($type:ident, $text:literal) => {
        const {
            $crate::$type::checked($text).expect(concat!(
                "`",
                $text,
                "` breaks the grammar of ",
                stringify!($type)
            ))
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kebab_names_follow_their_grammar() {
        assert_eq!(
            crate::name!(ScenarioId, "lower-mac-tx-ownership").as_str(),
            "lower-mac-tx-ownership"
        );
        assert!(InstanceName::parse("p2-step-bounds").is_ok());
        for name in [
            "",
            "Upper",
            "1st",
            "trailing-",
            "double--hyphen",
            "under_score",
            "dot.ted",
            &"a".repeat(65),
        ] {
            assert!(InstanceName::parse(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn names_end_at_their_length_limits() {
        assert!(InstanceName::parse(&"a".repeat(64)).is_ok());
        assert!(InstanceName::parse(&"a".repeat(65)).is_err());
        let segment = "a".repeat(47);
        assert!(SpanName::parse(&format!("{segment}.{}", "b".repeat(48))).is_ok());
        assert!(SpanName::parse(&format!("{segment}.{}", "b".repeat(49))).is_err());
    }

    #[test]
    fn a_dotted_segment_is_at_most_64_bytes() {
        assert!(SpanName::parse(&"a".repeat(64)).is_ok());
        assert!(SpanName::parse(&"a".repeat(65)).is_err());
        assert!(SpanName::parse(&format!("{}.b", "a".repeat(65))).is_err());
    }

    #[test]
    fn dotted_names_join_kebab_segments() {
        assert!(SpanName::parse("lower-mac.drain").is_ok());
        assert!(SpanName::parse("boot-to-hello").is_ok());
        for name in ["", ".a", "a.", "a..b", "a.B", "a.-b"] {
            assert!(SpanName::parse(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn a_parsed_name_reads_back_and_a_wrong_one_is_refused() {
        let name: MeasurementName = serde_json::from_str("\"connect.attempts\"").unwrap();
        assert_eq!(name, crate::name!(MeasurementName, "connect.attempts"));
        assert!(serde_json::from_str::<MeasurementName>("\"Connect\"").is_err());
    }

    #[test]
    fn contract_paths_name_a_package_and_items() {
        assert!(ContractName::parse("oer-ieee80211-lower-mac::LowerMacPort").is_ok());
        assert!(ContractName::parse("oer_radio_port::conformance::Harness").is_ok());
        for name in [
            "",
            "LowerMacPort",
            "oer::",
            "::Port",
            "Oer::Port",
            "oer::1Port",
            "oer:Port",
            "oer::Port::",
            "oer::a b",
        ] {
            assert!(ContractName::parse(name).is_err(), "{name:?}");
        }
    }
}
