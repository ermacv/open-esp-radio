#![forbid(unsafe_code)]
//! The host's complete HIL vocabulary, assembled from the owners' dictionaries.
mod image_keys;
pub use image_keys::DeviceImageKeys;
use oer_hil_protocol::MessageInfo;

/// Every known endpoint, event and image property.
pub fn registry() -> impl Iterator<Item = &'static MessageInfo> {
    oer_hil_protocol::registry().chain(
        [
            oer_ieee80211_control::MESSAGES,
            oer_network_control::MESSAGES,
            oer_bluetooth_control::MESSAGES,
            oer_ieee80211_conformance_control::MESSAGES,
            oer_esp32s31_phy_probe::MESSAGES,
            oer_esp32s31_instrumentation_hil::MESSAGES,
        ]
        .into_iter()
        .flatten(),
    )
}

/// Each owner's reviewed wire contract and its repository path.
pub const LOCKS: &[(&str, &str)] = &[
    (
        "protocols/hil/protocol/messages.lock",
        oer_hil_protocol::MESSAGES_LOCK,
    ),
    (
        "protocols/ieee80211/control/messages.lock",
        oer_ieee80211_control::MESSAGES_LOCK,
    ),
    (
        "protocols/network-control/messages.lock",
        oer_network_control::MESSAGES_LOCK,
    ),
    (
        "protocols/bluetooth/control/messages.lock",
        oer_bluetooth_control::MESSAGES_LOCK,
    ),
    (
        "protocols/ieee80211/conformance-control/messages.lock",
        oer_ieee80211_conformance_control::MESSAGES_LOCK,
    ),
    (
        "chips/esp32s31/phy-probe/messages.lock",
        oer_esp32s31_phy_probe::MESSAGES_LOCK,
    ),
    (
        "chips/esp32s31/instrumentation-hil/messages.lock",
        oer_esp32s31_instrumentation_hil::MESSAGES_LOCK,
    ),
];

/// Canonical wire contract, independent of the dictionaries' package boundaries.
pub fn messages_lock() -> Result<String, LockError> {
    merge_locks(LOCKS.iter().map(|(_, lock)| *lock))
}

/// Merge reviewed lock parts, retaining the framing version and sorting modules/messages.
/// Input comes from the owners' lock files; registry tests validate every part.
pub fn merge_locks<'a>(locks: impl IntoIterator<Item = &'a str>) -> Result<String, LockError> {
    let mut framing = None;
    let mut modules = std::collections::BTreeSet::new();
    let mut messages = std::collections::BTreeMap::new();
    let mut keys = std::collections::BTreeSet::new();
    for lock in locks {
        let mut section = "";
        let mut part_framing = None;
        let mut part_modules = false;
        let mut part_messages = false;
        for line in lock
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
        {
            match line {
                "[modules]" if section.is_empty() && part_framing.is_some() => section = line,
                "[messages]" if section == "[modules]" => section = line,
                _ if line.starts_with("framing ")
                    && section.is_empty()
                    && part_framing.is_none() =>
                {
                    let version = line
                        .strip_prefix("framing ")
                        .and_then(|v| v.parse::<u8>().ok())
                        .ok_or_else(|| LockError::InvalidLine(line.to_owned()))?;
                    if framing.is_some_and(|expected| expected != version) {
                        return Err(LockError::FramingMismatch);
                    }
                    framing = Some(version);
                    part_framing = Some(version);
                }
                _ if section == "[modules]" => {
                    if !line.split_whitespace().all(valid_segment) || !modules.insert(line) {
                        return Err(LockError::InvalidLine(line.to_owned()));
                    }
                    part_modules = true;
                }
                _ if section == "[messages]" => {
                    let mut parts = line.split_whitespace();
                    let path = parts
                        .next()
                        .ok_or_else(|| LockError::InvalidLine(line.to_owned()))?;
                    let key = parts
                        .next()
                        .ok_or_else(|| LockError::InvalidLine(line.to_owned()))?;
                    let kind = parts
                        .next()
                        .ok_or_else(|| LockError::InvalidLine(line.to_owned()))?;
                    if !path.split('/').all(valid_segment)
                        || !path.contains('/')
                        || key.len() != 16
                        || !key.bytes().all(|b| b.is_ascii_hexdigit())
                        || !matches!(kind, "endpoint" | "topic" | "property")
                        || parts.next().is_some()
                        || messages.insert(path, line).is_some()
                        || !keys.insert(key)
                    {
                        return Err(LockError::InvalidLine(line.to_owned()));
                    }
                    part_messages = true;
                }
                _ => return Err(LockError::InvalidLine(line.to_owned())),
            }
        }
        if part_framing.is_none() || !part_modules || !part_messages || section != "[messages]" {
            return Err(LockError::Incomplete);
        }
    }
    let framing = framing.ok_or(LockError::Incomplete)?;
    let mut out = String::from(
        "# The HIL wire of this revision.\n# The registry test prints the complete replacement when it differs.\n",
    );
    out.push_str(&format!("framing {framing}\n"));
    out.push_str("[modules]\n");
    for line in modules {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("[messages]\n");
    for line in messages.values() {
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A source tree whose lock parts cannot describe one unambiguous wire.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LockError {
    Incomplete,
    FramingMismatch,
    InvalidLine(String),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid HIL message locks: {self:?}")
    }
}

impl std::error::Error for LockError {}
