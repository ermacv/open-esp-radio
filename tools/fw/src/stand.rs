//! The owner's stand file: which boards the stand has and which hub port
//! powers each.
//!
//! Only `schema`, `[[hub]]` and `[[board]]` are read. The file also holds the
//! stand's network credentials, so nothing else is deserialized, and a parse
//! error names its line, never the file's text.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Result;

/// The schema this tool reads.
pub const SCHEMA: u32 = 1;

/// Overrides the stand file's path.
pub const STAND_FILE_ENV: &str = "OER_STAND_FILE";

/// The stand file's path: [`STAND_FILE_ENV`], or
/// `~/.config/open-esp-radio/stand.toml`.
pub fn path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(STAND_FILE_ENV) {
        return Ok(PathBuf::from(path));
    }
    let config = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(config) if !config.is_empty() => PathBuf::from(config),
        _ => PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?).join(".config"),
    };
    Ok(config.join("open-esp-radio/stand.toml"))
}

/// A USB hub whose port power `uhubctl` switches.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Hub {
    pub id: String,
    /// The hub's USB 2 location (`uhubctl -l`).
    pub usb2: String,
    /// Its USB 3 companion's location, when it has one.
    #[serde(default)]
    pub usb3: Option<String>,
    /// Ports that carry further hubs: never switched off.
    #[serde(default)]
    pub protected: Vec<u32>,
    /// Ports whose power may be switched.
    #[serde(default)]
    pub switchable: Vec<u32>,
}

/// Where a board's USB cable plugs in.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct HubPort {
    pub hub: String,
    pub port: u32,
}

/// A board of the stand.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Board {
    pub id: String,
    /// The USB serial number of its USB Serial/JTAG: the chip's MAC.
    #[serde(rename = "usb-serial")]
    pub mac: String,
    pub chip: String,
    #[serde(default)]
    pub port: Option<HubPort>,
}

/// The parts of the stand file this tool reads.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Stand {
    pub schema: u32,
    #[serde(default, rename = "hub")]
    pub hubs: Vec<Hub>,
    #[serde(default, rename = "board")]
    pub boards: Vec<Board>,
}

impl Stand {
    /// The stand file at `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("read stand file {}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("stand file {}: {error}", path.display()).into())
    }

    /// The stand described by `text`.
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let stand: Self = toml::from_str(text).map_err(|error| {
            // The error's own rendering quotes the offending line, which may
            // hold a credential: report its position and message only.
            let line = error
                .span()
                .map(|span| text[..span.start.min(text.len())].matches('\n').count() + 1);
            match line {
                Some(line) => format!("line {line}: {}", error.message()),
                None => error.message().to_owned(),
            }
        })?;
        if stand.schema != SCHEMA {
            return Err(format!(
                "schema {} is not the schema {SCHEMA} this tool reads",
                stand.schema
            ));
        }
        Ok(stand)
    }

    /// The board named `selector`: its id, or its MAC in any case.
    pub fn board(&self, selector: &str) -> Option<&Board> {
        self.boards
            .iter()
            .find(|board| board.id == selector || board.mac.eq_ignore_ascii_case(selector))
    }

    /// The board whose MAC is `mac`.
    pub fn board_by_mac(&self, mac: &str) -> Option<&Board> {
        self.boards
            .iter()
            .find(|board| board.mac.eq_ignore_ascii_case(mac))
    }

    pub fn hub(&self, id: &str) -> Option<&Hub> {
        self.hubs.iter().find(|hub| hub.id == id)
    }

    /// The hub and port that power `board`, if its port may be switched.
    pub fn switchable_port(&self, board: &Board) -> std::result::Result<(&Hub, u32), String> {
        let port = board
            .port
            .as_ref()
            .ok_or_else(|| format!("board {} names no hub port", board.id))?;
        let hub = self
            .hub(&port.hub)
            .ok_or_else(|| format!("board {} names the unknown hub {}", board.id, port.hub))?;
        if hub.protected.contains(&port.port) {
            return Err(format!(
                "port {} of hub {} is protected: it carries further hubs",
                port.port, hub.id
            ));
        }
        if !hub.switchable.contains(&port.port) {
            return Err(format!(
                "port {} of hub {} is not switchable",
                port.port, hub.id
            ));
        }
        Ok((hub, port.port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAND: &str = r#"
schema = 1

[stand]
secret = "never read"

[[hub]]
id = "top"
usb2 = "3-8"
usb3 = "4-1"
protected = [3]
switchable = [1, 4]
buttons = { "1" = 1 }

[[board]]
id = "s31-a"
usb-serial = "30:ED:A0:F3:F6:D0"
chip = "esp32s31"
port = { hub = "top", port = 4 }
reset = ["power"]

[[board]]
id = "behind-hub"
usb-serial = "AA:BB:CC:DD:EE:FF"
chip = "esp32s31"
port = { hub = "top", port = 3 }

[[station]]
password = "hunter2"
"#;

    #[test]
    fn reads_hubs_and_boards_and_ignores_the_rest() {
        let stand = Stand::parse(STAND).unwrap();
        assert_eq!(stand.hubs.len(), 1);
        assert_eq!(stand.hubs[0].usb3.as_deref(), Some("4-1"));
        let board = stand.board("s31-a").unwrap();
        assert_eq!(board.mac, "30:ED:A0:F3:F6:D0");
        assert_eq!(stand.board("30:ed:a0:f3:f6:d0"), Some(board));
        assert_eq!(stand.board_by_mac("30:ED:A0:F3:F6:D0"), Some(board));
        assert_eq!(stand.board("nope"), None);
    }

    #[test]
    fn only_a_switchable_unprotected_port_is_switched() {
        let stand = Stand::parse(STAND).unwrap();
        let (hub, port) = stand
            .switchable_port(stand.board("s31-a").unwrap())
            .unwrap();
        assert_eq!((hub.id.as_str(), port), ("top", 4));
        let error = stand
            .switchable_port(stand.board("behind-hub").unwrap())
            .unwrap_err();
        assert!(error.contains("protected"), "{error}");
    }

    #[test]
    fn a_parse_error_does_not_quote_the_file() {
        let error = Stand::parse("schema = 1\n[station]\npassword = hunter2\n").unwrap_err();
        assert!(!error.contains("hunter2"), "{error}");
        assert!(error.starts_with("line 3"), "{error}");
    }

    #[test]
    fn another_schema_is_refused() {
        assert!(
            Stand::parse("schema = 2\n")
                .unwrap_err()
                .contains("schema 2")
        );
    }
}
