use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};

const OCCURRENCE_DOMAIN_SEPARATOR: &[u8] = b"blobray/revision-occurrence/v1\0";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityError(String);

impl IdentityError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for IdentityError {}

/// A revision-independent, lower-case path owned by one semantic domain.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SemanticPath(String);

impl SemanticPath {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentityError> {
        let value = value.into();
        validate_semantic_path(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SemanticPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for SemanticPath {
    type Err = IdentityError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Serialize for SemanticPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SemanticPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntityDomain {
    Function,
    MemoryObject,
    Register,
    RegisterField,
    Interface,
    InterfaceSlot,
    LogicalType,
    EventRoute,
}

impl EntityDomain {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::MemoryObject => "memory-object",
            Self::Register => "register",
            Self::RegisterField => "register-field",
            Self::Interface => "interface",
            Self::InterfaceSlot => "interface-slot",
            Self::LogicalType => "logical-type",
            Self::EventRoute => "event-route",
        }
    }
}

impl fmt::Display for EntityDomain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

impl FromStr for EntityDomain {
    type Err = IdentityError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "function" => Ok(Self::Function),
            "memory-object" => Ok(Self::MemoryObject),
            "register" => Ok(Self::Register),
            "register-field" => Ok(Self::RegisterField),
            "interface" => Ok(Self::Interface),
            "interface-slot" => Ok(Self::InterfaceSlot),
            "logical-type" => Ok(Self::LogicalType),
            "event-route" => Ok(Self::EventRoute),
            _ => Err(IdentityError::new(format!(
                "unknown entity domain {value:?}"
            ))),
        }
    }
}

/// One semantic entity whose identity does not contain a blob address.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticEntityId {
    Function(SemanticPath),
    MemoryObject(SemanticPath),
    Register {
        chip: String,
        address_space: String,
        address: u64,
        width: u32,
    },
    RegisterField {
        chip: String,
        address_space: String,
        address: u64,
        register_width: u32,
        bit_offset: u32,
        bit_width: u32,
    },
    Interface(SemanticPath),
    InterfaceSlot {
        interface: SemanticPath,
        offset: u64,
        width: u32,
    },
    LogicalType(SemanticPath),
    EventRoute(SemanticPath),
}

impl SemanticEntityId {
    pub fn function(path: impl Into<String>) -> Result<Self, IdentityError> {
        Ok(Self::Function(SemanticPath::new(path)?))
    }

    pub fn memory_object(path: impl Into<String>) -> Result<Self, IdentityError> {
        Ok(Self::MemoryObject(SemanticPath::new(path)?))
    }

    pub fn register(
        chip: impl Into<String>,
        address_space: impl Into<String>,
        address: u64,
        width: u32,
    ) -> Result<Self, IdentityError> {
        let chip = chip.into();
        let address_space = address_space.into();
        validate_component("chip", &chip)?;
        validate_component("address space", &address_space)?;
        validate_width("register width", width)?;
        Ok(Self::Register {
            chip,
            address_space,
            address,
            width,
        })
    }

    pub fn register_field(
        chip: impl Into<String>,
        address_space: impl Into<String>,
        address: u64,
        register_width: u32,
        bit_offset: u32,
        bit_width: u32,
    ) -> Result<Self, IdentityError> {
        let chip = chip.into();
        let address_space = address_space.into();
        validate_component("chip", &chip)?;
        validate_component("address space", &address_space)?;
        validate_width("register width", register_width)?;
        validate_width("field width", bit_width)?;
        let end = bit_offset
            .checked_add(bit_width)
            .ok_or_else(|| IdentityError::new("register field range overflows u32"))?;
        if end > register_width {
            return Err(IdentityError::new(format!(
                "register field bits {bit_offset}..{end} exceed register width {register_width}"
            )));
        }
        Ok(Self::RegisterField {
            chip,
            address_space,
            address,
            register_width,
            bit_offset,
            bit_width,
        })
    }

    pub fn interface(path: impl Into<String>) -> Result<Self, IdentityError> {
        Ok(Self::Interface(SemanticPath::new(path)?))
    }

    pub fn interface_slot(
        interface: impl Into<String>,
        offset: u64,
        width: u32,
    ) -> Result<Self, IdentityError> {
        validate_width("interface slot width", width)?;
        Ok(Self::InterfaceSlot {
            interface: SemanticPath::new(interface)?,
            offset,
            width,
        })
    }

    pub fn logical_type(path: impl Into<String>) -> Result<Self, IdentityError> {
        Ok(Self::LogicalType(SemanticPath::new(path)?))
    }

    pub fn event_route(path: impl Into<String>) -> Result<Self, IdentityError> {
        Ok(Self::EventRoute(SemanticPath::new(path)?))
    }

    pub const fn domain(&self) -> EntityDomain {
        match self {
            Self::Function(_) => EntityDomain::Function,
            Self::MemoryObject(_) => EntityDomain::MemoryObject,
            Self::Register { .. } => EntityDomain::Register,
            Self::RegisterField { .. } => EntityDomain::RegisterField,
            Self::Interface(_) => EntityDomain::Interface,
            Self::InterfaceSlot { .. } => EntityDomain::InterfaceSlot,
            Self::LogicalType(_) => EntityDomain::LogicalType,
            Self::EventRoute(_) => EntityDomain::EventRoute,
        }
    }
}

impl fmt::Display for SemanticEntityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Function(path) => write!(formatter, "function:{path}"),
            Self::MemoryObject(path) => write!(formatter, "memory-object:{path}"),
            Self::Register {
                chip,
                address_space,
                address,
                width,
            } => write!(
                formatter,
                "register:{chip}/{address_space}/0x{address:x}/{width}"
            ),
            Self::RegisterField {
                chip,
                address_space,
                address,
                register_width,
                bit_offset,
                bit_width,
            } => write!(
                formatter,
                "register-field:{chip}/{address_space}/0x{address:x}/{register_width}/{bit_offset}/{bit_width}"
            ),
            Self::Interface(path) => write!(formatter, "interface:{path}"),
            Self::InterfaceSlot {
                interface,
                offset,
                width,
            } => write!(formatter, "interface-slot:{interface}/0x{offset:x}/{width}"),
            Self::LogicalType(path) => write!(formatter, "logical-type:{path}"),
            Self::EventRoute(path) => write!(formatter, "event-route:{path}"),
        }
    }
}

impl FromStr for SemanticEntityId {
    type Err = IdentityError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parsed = if let Some(path) = value.strip_prefix("function:") {
            Self::function(path)
        } else if let Some(path) = value.strip_prefix("memory-object:") {
            Self::memory_object(path)
        } else if let Some(rest) = value.strip_prefix("register-field:") {
            parse_register_field(rest)
        } else if let Some(rest) = value.strip_prefix("register:") {
            parse_register(rest)
        } else if let Some(rest) = value.strip_prefix("interface-slot:") {
            parse_interface_slot(rest)
        } else if let Some(path) = value.strip_prefix("interface:") {
            Self::interface(path)
        } else if let Some(path) = value.strip_prefix("logical-type:") {
            Self::logical_type(path)
        } else if let Some(path) = value.strip_prefix("event-route:") {
            Self::event_route(path)
        } else {
            return Err(IdentityError::new(format!(
                "semantic entity {value:?} has no recognized domain prefix"
            )));
        }?;
        require_canonical(value, &parsed.to_string())?;
        Ok(parsed)
    }
}

impl Serialize for SemanticEntityId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for SemanticEntityId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// Exact content identity of one artifact in a project composition.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ArtifactIdentity {
    source: String,
    sha256: String,
}

impl ArtifactIdentity {
    pub fn new(
        source: impl Into<String>,
        sha256: impl Into<String>,
    ) -> Result<Self, IdentityError> {
        let source = source.into();
        let sha256 = sha256.into();
        validate_stable_path("artifact source", &source)?;
        validate_sha256(&sha256)?;
        Ok(Self { source, sha256 })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

impl fmt::Display for ArtifactIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "artifact:sha256:{}/{}", self.sha256, self.source)
    }
}

impl FromStr for ArtifactIdentity {
    type Err = IdentityError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let rest = value.strip_prefix("artifact:sha256:").ok_or_else(|| {
            IdentityError::new("artifact identity must start with artifact:sha256:")
        })?;
        if rest.len() < 66 || rest.as_bytes().get(64) != Some(&b'/') {
            return Err(IdentityError::new(
                "artifact identity must contain a 64-digit digest and source",
            ));
        }
        let parsed = Self::new(&rest[65..], &rest[..64])?;
        require_canonical(value, &parsed.to_string())?;
        Ok(parsed)
    }
}

impl<'de> Deserialize<'de> for ArtifactIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case", deny_unknown_fields)]
        struct Wire {
            source: String,
            sha256: String,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.source, wire.sha256).map_err(de::Error::custom)
    }
}

/// Blob-local identity for an observation that may not yet have a semantic ID.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RevisionOccurrenceId {
    domain: EntityDomain,
    digest: String,
}

impl RevisionOccurrenceId {
    pub fn derive(
        domain: EntityDomain,
        artifacts: &[ArtifactIdentity],
        locator: &str,
    ) -> Result<Self, IdentityError> {
        if artifacts.is_empty() {
            return Err(IdentityError::new(
                "revision occurrence requires at least one exact artifact identity",
            ));
        }
        validate_locator(locator)?;
        let mut artifacts = artifacts.to_vec();
        artifacts.sort();
        if artifacts.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(IdentityError::new(
                "revision occurrence artifact identities must be unique",
            ));
        }

        let mut hash = Sha256::new();
        hash.update(OCCURRENCE_DOMAIN_SEPARATOR);
        update_framed(&mut hash, domain.label().as_bytes());
        hash.update((artifacts.len() as u64).to_be_bytes());
        for artifact in artifacts {
            update_framed(&mut hash, artifact.source().as_bytes());
            update_framed(&mut hash, artifact.sha256().as_bytes());
        }
        update_framed(&mut hash, locator.as_bytes());
        let digest = hex_digest(hash.finalize().as_slice());
        Ok(Self { domain, digest })
    }

    pub const fn domain(&self) -> EntityDomain {
        self.domain
    }

    pub fn sha256(&self) -> &str {
        &self.digest
    }
}

impl fmt::Display for RevisionOccurrenceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "occurrence:{}:sha256:{}",
            self.domain, self.digest
        )
    }
}

impl FromStr for RevisionOccurrenceId {
    type Err = IdentityError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let rest = value
            .strip_prefix("occurrence:")
            .ok_or_else(|| IdentityError::new("revision occurrence must start with occurrence:"))?;
        let (domain, digest) = rest
            .split_once(":sha256:")
            .ok_or_else(|| IdentityError::new("revision occurrence must include :sha256:"))?;
        let domain = domain.parse()?;
        validate_sha256(digest)?;
        let parsed = Self {
            domain,
            digest: digest.to_owned(),
        };
        require_canonical(value, &parsed.to_string())?;
        Ok(parsed)
    }
}

impl Serialize for RevisionOccurrenceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for RevisionOccurrenceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

pub(crate) fn validate_stable_id(field: &str, value: &str) -> Result<(), IdentityError> {
    if value.is_empty()
        || value.len() > 160
        || !value.is_ascii()
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'-' | b'_' | b':' | b'/' | b'+')
        })
        || !value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        || value.contains("//")
        || value
            .split('/')
            .any(|component| matches!(component, "." | ".."))
    {
        return Err(IdentityError::new(format!(
            "{field} must be a canonical 1..160 lower-case stable ID"
        )));
    }
    Ok(())
}

pub(crate) fn validate_sha256(value: &str) -> Result<(), IdentityError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(IdentityError::new(
            "SHA-256 digest must contain exactly 64 lower-case hexadecimal digits",
        ));
    }
    Ok(())
}

fn validate_semantic_path(value: &str) -> Result<(), IdentityError> {
    validate_stable_path("semantic path", value)?;
    if value.contains('@') || value.split('/').any(|part| part.starts_with("0x")) {
        return Err(IdentityError::new(
            "semantic path must not contain a revision address or @ address qualifier",
        ));
    }
    Ok(())
}

fn validate_stable_path(field: &str, value: &str) -> Result<(), IdentityError> {
    validate_stable_id(field, value)?;
    if value.starts_with('/') || value.ends_with('/') {
        return Err(IdentityError::new(format!(
            "{field} must be a relative canonical path"
        )));
    }
    Ok(())
}

fn validate_component(field: &str, value: &str) -> Result<(), IdentityError> {
    validate_stable_id(field, value)?;
    if value.contains(['/', ':']) {
        return Err(IdentityError::new(format!(
            "{field} must be one stable path component"
        )));
    }
    Ok(())
}

fn validate_width(field: &str, value: u32) -> Result<(), IdentityError> {
    if value == 0 {
        return Err(IdentityError::new(format!("{field} must be non-zero")));
    }
    Ok(())
}

fn validate_locator(value: &str) -> Result<(), IdentityError> {
    if value.is_empty()
        || value.len() > 1024
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(IdentityError::new(
            "occurrence locator must be 1..1024 non-control characters without surrounding whitespace",
        ));
    }
    Ok(())
}

fn parse_register(value: &str) -> Result<SemanticEntityId, IdentityError> {
    let parts = value.split('/').collect::<Vec<_>>();
    if parts.len() != 4 {
        return Err(IdentityError::new(
            "register identity requires chip/address-space/address/width",
        ));
    }
    SemanticEntityId::register(
        parts[0],
        parts[1],
        parse_hex("register address", parts[2])?,
        parse_u32("register width", parts[3])?,
    )
}

fn parse_register_field(value: &str) -> Result<SemanticEntityId, IdentityError> {
    let parts = value.split('/').collect::<Vec<_>>();
    if parts.len() != 6 {
        return Err(IdentityError::new(
            "register field identity requires chip/address-space/address/register-width/bit-offset/bit-width",
        ));
    }
    SemanticEntityId::register_field(
        parts[0],
        parts[1],
        parse_hex("register address", parts[2])?,
        parse_u32("register width", parts[3])?,
        parse_u32("field bit offset", parts[4])?,
        parse_u32("field bit width", parts[5])?,
    )
}

fn parse_interface_slot(value: &str) -> Result<SemanticEntityId, IdentityError> {
    let mut parts = value.rsplitn(3, '/');
    let width = parts
        .next()
        .ok_or_else(|| IdentityError::new("interface slot identity requires path/offset/width"))?;
    let offset = parts
        .next()
        .ok_or_else(|| IdentityError::new("interface slot identity requires path/offset/width"))?;
    let path = parts
        .next()
        .ok_or_else(|| IdentityError::new("interface slot identity requires path/offset/width"))?;
    SemanticEntityId::interface_slot(
        path,
        parse_hex("interface slot offset", offset)?,
        parse_u32("interface slot width", width)?,
    )
}

fn parse_hex(field: &str, value: &str) -> Result<u64, IdentityError> {
    let digits = value
        .strip_prefix("0x")
        .filter(|digits| !digits.is_empty())
        .ok_or_else(|| IdentityError::new(format!("{field} must start with 0x")))?;
    if (digits.len() > 1 && digits.starts_with('0'))
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(IdentityError::new(format!(
            "{field} is not canonical lower-case hexadecimal"
        )));
    }
    u64::from_str_radix(digits, 16).map_err(|_| IdentityError::new(format!("{field} exceeds u64")))
}

fn parse_u32(field: &str, value: &str) -> Result<u32, IdentityError> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(IdentityError::new(format!(
            "{field} is not canonical decimal"
        )));
    }
    value
        .parse()
        .map_err(|_| IdentityError::new(format!("{field} exceeds u32")))
}

fn require_canonical(input: &str, rendered: &str) -> Result<(), IdentityError> {
    if input != rendered {
        return Err(IdentityError::new(format!(
            "identity is not canonical; expected {rendered:?}"
        )));
    }
    Ok(())
}

fn update_framed(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value);
}

fn hex_digest(value: &[u8]) -> String {
    let mut output = String::with_capacity(value.len() * 2);
    for byte in value {
        use fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    fn artifact(source: &str, byte: char) -> ArtifactIdentity {
        ArtifactIdentity::new(source, digest(byte)).unwrap()
    }

    #[test]
    fn every_semantic_entity_has_one_canonical_string_and_serde_form() {
        let values = [
            (
                SemanticEntityId::function("esp-idf/wifi/rx").unwrap(),
                "function:esp-idf/wifi/rx",
            ),
            (
                SemanticEntityId::memory_object("esp-idf/ble/controller-state").unwrap(),
                "memory-object:esp-idf/ble/controller-state",
            ),
            (
                SemanticEntityId::register("chip-alpha", "radio", 0x600a_0034, 32).unwrap(),
                "register:chip-alpha/radio/0x600a0034/32",
            ),
            (
                SemanticEntityId::register_field("chip-alpha", "radio", 0x600a_0034, 32, 8, 3)
                    .unwrap(),
                "register-field:chip-alpha/radio/0x600a0034/32/8/3",
            ),
            (
                SemanticEntityId::interface("esp-idf/wifi-osi-v9").unwrap(),
                "interface:esp-idf/wifi-osi-v9",
            ),
            (
                SemanticEntityId::interface_slot("esp-idf/wifi-osi-v9", 0x18, 32).unwrap(),
                "interface-slot:esp-idf/wifi-osi-v9/0x18/32",
            ),
            (
                SemanticEntityId::logical_type("esp-idf/wifi-buffer").unwrap(),
                "logical-type:esp-idf/wifi-buffer",
            ),
            (
                SemanticEntityId::event_route("chip-alpha/radio/rx-done").unwrap(),
                "event-route:chip-alpha/radio/rx-done",
            ),
        ];
        for (value, expected) in values {
            let encoded = value.to_string();
            assert_eq!(encoded, expected);
            assert_eq!(encoded.parse::<SemanticEntityId>().unwrap(), value);
            let json = serde_json::to_string(&value).unwrap();
            assert_eq!(
                serde_json::from_str::<SemanticEntityId>(&json).unwrap(),
                value
            );
            assert_eq!(
                value.domain().to_string(),
                encoded.split(':').next().unwrap()
            );
        }
    }

    #[test]
    fn semantic_paths_reject_revision_addresses_and_noncanonical_spelling() {
        for invalid in [
            "wifi/rx@0x40001000",
            "wifi/0x40001000",
            "WiFi/rx",
            "/wifi/rx",
            "wifi//rx",
            "wifi/../rx",
            "wifi/rx ",
        ] {
            assert!(SemanticEntityId::function(invalid).is_err(), "{invalid}");
        }
        for invalid in [
            "register:chip-alpha/radio/0x0600a0034/32",
            "register:chip-alpha/radio/0x600A0034/32",
            "register:chip-alpha/radio/0x600a0034/032",
            "function:wifi/rx@0x1",
            "function:WiFi/rx",
        ] {
            assert!(invalid.parse::<SemanticEntityId>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn typed_ranges_fail_closed() {
        assert!(SemanticEntityId::register("chip-alpha", "radio", 0, 0).is_err());
        assert!(SemanticEntityId::register_field("chip-alpha", "radio", 0, 32, 31, 2).is_err());
        assert!(SemanticEntityId::interface_slot("sdk/interface", 0, 0).is_err());
    }

    #[test]
    fn artifact_identity_is_strict_and_round_trips() {
        let value = artifact("esp-idf/lib/net80211.a", 'a');
        assert_eq!(
            value.to_string().parse::<ArtifactIdentity>().unwrap(),
            value
        );
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(
            serde_json::from_str::<ArtifactIdentity>(&json).unwrap(),
            value
        );
        assert!(ArtifactIdentity::new("/absolute/blob.a", digest('a')).is_err());
        assert!(ArtifactIdentity::new("blob.a", digest('A')).is_err());
        assert!(
            serde_json::from_str::<ArtifactIdentity>(&format!(
                "{{\"source\":\"blob.a\",\"sha256\":\"{}\",\"legacy\":true}}",
                digest('a')
            ))
            .is_err()
        );
    }

    #[test]
    fn occurrence_is_artifact_order_invariant_and_round_trips() {
        let left = artifact("sdk/lib/a.a", 'a');
        let right = artifact("sdk/lib/b.a", 'b');
        let first = RevisionOccurrenceId::derive(
            EntityDomain::Function,
            &[left.clone(), right.clone()],
            "text+0x18",
        )
        .unwrap();
        let reversed =
            RevisionOccurrenceId::derive(EntityDomain::Function, &[right, left], "text+0x18")
                .unwrap();
        assert_eq!(first, reversed);
        assert_eq!(
            first.sha256(),
            concat!(
                "f26a87113f30a110f0f75fd9e229cac2",
                "ff7feb9ed6878b3dfbbbc15dfb225c15"
            )
        );
        assert_eq!(
            first.to_string().parse::<RevisionOccurrenceId>().unwrap(),
            first
        );
        let json = serde_json::to_string(&first).unwrap();
        assert_eq!(
            serde_json::from_str::<RevisionOccurrenceId>(&json).unwrap(),
            first
        );
    }

    #[test]
    fn occurrence_hash_changes_for_every_identity_dimension() {
        let artifact_a = artifact("sdk/lib/a.a", 'a');
        let artifact_b = artifact("sdk/lib/a.a", 'b');
        let baseline = RevisionOccurrenceId::derive(
            EntityDomain::Function,
            std::slice::from_ref(&artifact_a),
            "text+0x18",
        )
        .unwrap();
        let domain = RevisionOccurrenceId::derive(
            EntityDomain::Interface,
            std::slice::from_ref(&artifact_a),
            "text+0x18",
        )
        .unwrap();
        let artifact =
            RevisionOccurrenceId::derive(EntityDomain::Function, &[artifact_b], "text+0x18")
                .unwrap();
        let locator =
            RevisionOccurrenceId::derive(EntityDomain::Function, &[artifact_a], "text+0x1c")
                .unwrap();
        assert_ne!(baseline, domain);
        assert_ne!(baseline, artifact);
        assert_ne!(baseline, locator);
    }

    #[test]
    fn occurrence_rejects_inexact_or_duplicate_inputs_and_legacy_strings() {
        let artifact = artifact("sdk/lib/a.a", 'a');
        assert!(RevisionOccurrenceId::derive(EntityDomain::Function, &[], "text+0x18").is_err());
        assert!(
            RevisionOccurrenceId::derive(
                EntityDomain::Function,
                &[artifact.clone(), artifact],
                "text+0x18"
            )
            .is_err()
        );
        assert!("function:deadbeef".parse::<RevisionOccurrenceId>().is_err());
        assert!(
            format!("occurrence:function:sha256:{}", digest('A'))
                .parse::<RevisionOccurrenceId>()
                .is_err()
        );
    }
}
