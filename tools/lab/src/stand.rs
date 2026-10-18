//! The owner's stand file (README.md): which boards the stand has and
//! which hub port powers each, and the stand router.
//!
//! This crate owns the whole file; each consumer's slice deserializes only
//! its fields and ignores the rest: `schema`, `[[hub]]` and `[[board]]` for
//! the boards, and for W1 `[station].ssid` and of `[station_fixture]`
//! `kind`, `ssh_target`, `wireless_interface`, `radio`, `ap_section` and
//! `channel`. `[station].passphrase` and `[access_point]` are never read. A
//! file whose mode is not 0600 is refused unread, and a parse error names
//! its line and never a value. The SSID, the SSH target and the interface
//! are [`Secret`]s: their `Debug` shows nothing of them.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Result;

/// The schema this tool reads.
pub const SCHEMA: u32 = 1;

/// The only mode a stand file may have.
pub const MODE: u32 = 0o600;

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

/// A value of the stand file that is never shown: its `Debug` is
/// `<hidden>`, and it has no `Display`.
#[derive(Clone, Deserialize, Eq, PartialEq)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// The value, for the one place that uses it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<hidden>")
    }
}

/// `[station]`: only its SSID. The passphrase beside it is not read.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct Station {
    #[serde(default)]
    pub ssid: Option<Secret>,
}

/// `[station_fixture]`: the router that serves the stand's AP.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct StationFixture {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub ssh_target: Option<Secret>,
    #[serde(default)]
    pub wireless_interface: Option<Secret>,
    /// The UCI `wifi-device` section.
    #[serde(default)]
    pub radio: Option<String>,
    /// The UCI `wifi-iface` section of the stand's AP.
    #[serde(default)]
    pub ap_section: Option<String>,
    #[serde(default)]
    pub channel: Option<u8>,
}

/// The fixture kind image `w1` drives.
pub const OPEN_WRT: &str = "open-wrt";

/// What image `w1` needs of the stand router.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Router<'a> {
    pub ssid: &'a Secret,
    pub ssh_target: &'a Secret,
    pub wireless_interface: &'a Secret,
    pub radio: &'a str,
    pub ap_section: &'a str,
    pub channel: u8,
}

/// The parts of the stand file this tool reads.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Stand {
    pub schema: u32,
    #[serde(default, rename = "hub")]
    pub hubs: Vec<Hub>,
    #[serde(default, rename = "board")]
    pub boards: Vec<Board>,
    #[serde(default)]
    pub station: Station,
    #[serde(default)]
    pub station_fixture: StationFixture,
}

impl Stand {
    /// The stand file at `path`.
    /// The stand file holds the stand's credentials: one that others may
    /// read, mode other than 0600, is refused unread.
    pub fn load(path: &Path) -> Result<Self> {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(path)
            .map_err(|error| format!("read stand file {}: {error}", path.display()))?
            .permissions()
            .mode()
            & 0o777;
        if mode != MODE {
            return Err(format!(
                "stand file {} has mode {mode:04o}: it holds the stand's credentials and must be                  {MODE:04o} (`chmod 600 {}`)",
                path.display(),
                path.display()
            )
            .into());
        }
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
            let message = shown_message(error.message());
            match line {
                Some(line) => format!("line {line}: {message}"),
                None => message,
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

    /// The stand router of image `w1`: every key present, `kind` `open-wrt`.
    /// An error names the missing key, never a value.
    pub fn router(&self) -> std::result::Result<Router<'_>, String> {
        let fixture = &self.station_fixture;
        let missing = |key: &str| format!("the stand file has no {key}, which image w1 needs");
        match fixture.kind.as_deref() {
            Some(OPEN_WRT) => {}
            Some(_) => {
                return Err(format!(
                    "[station_fixture].kind is not `{OPEN_WRT}`: image w1 drives an OpenWrt router"
                ));
            }
            None => return Err(missing("[station_fixture].kind")),
        }
        Ok(Router {
            ssid: self
                .station
                .ssid
                .as_ref()
                .ok_or_else(|| missing("[station].ssid"))?,
            ssh_target: fixture
                .ssh_target
                .as_ref()
                .ok_or_else(|| missing("[station_fixture].ssh_target"))?,
            wireless_interface: fixture
                .wireless_interface
                .as_ref()
                .ok_or_else(|| missing("[station_fixture].wireless_interface"))?,
            radio: fixture
                .radio
                .as_deref()
                .ok_or_else(|| missing("[station_fixture].radio"))?,
            ap_section: fixture
                .ap_section
                .as_deref()
                .ok_or_else(|| missing("[station_fixture].ap_section"))?,
            channel: fixture
                .channel
                .ok_or_else(|| missing("[station_fixture].channel"))?,
        })
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

/// A deserializer's `message`, without the value it may quote: serde's
/// `invalid type: string "…", expected …` keeps only what was expected.
fn shown_message(message: &str) -> String {
    for prefix in ["invalid type", "invalid value", "invalid length"] {
        if message.starts_with(prefix) {
            return match message.split_once(", expected ") {
                Some((_, expected)) => format!("{prefix}, expected {expected}"),
                None => prefix.to_owned(),
            };
        }
    }
    message.to_owned()
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
usb-serial = "02:00:00:00:01:D0"
chip = "esp32s31"
port = { hub = "top", port = 4 }
reset = ["power"]

[[board]]
id = "behind-hub"
usb-serial = "AA:BB:CC:DD:EE:FF"
chip = "esp32s31"
port = { hub = "top", port = 3 }

[station]
ssid = "Made Up Lab Net"
passphrase = "made-up-passphrase"

[station_fixture]
kind = "open-wrt"
ssh_target = "made-up-router.lab"
wireless_interface = "phy7-ap3"
radio = "radio9"
ap_section = "lab_ap"
channel = 6
ingress_interface = "made-up-ingress"

[access_point]
ssid = "made-up-ap-ssid"
passphrase = "made-up-ap-passphrase"
"#;

    /// Every value the file holds that may never be shown.
    const SECRETS: [&str; 7] = [
        "Made Up Lab Net",
        "made-up-passphrase",
        "made-up-router",
        "phy7-ap3",
        "made-up-ingress",
        "made-up-ap-ssid",
        "made-up-ap-passphrase",
    ];

    #[test]
    fn reads_the_router_and_nothing_secret_is_kept_or_shown() {
        let stand = Stand::parse(STAND).unwrap();
        let router = stand.router().unwrap();
        assert_eq!(router.ssid.expose(), "Made Up Lab Net");
        assert_eq!(router.ssh_target.expose(), "made-up-router.lab");
        assert_eq!(router.wireless_interface.expose(), "phy7-ap3");
        assert_eq!(
            (router.radio, router.ap_section, router.channel),
            ("radio9", "lab_ap", 6)
        );
        let shown = format!("{stand:?}\n{router:?}");
        for secret in SECRETS {
            assert!(!shown.contains(secret), "{secret}: {shown}");
        }
        // The passphrase is not deserialized: the parsed stand is the same
        // without it.
        let without = STAND.replace("passphrase = \"made-up-passphrase\"\n", "");
        assert_eq!(Stand::parse(&without).unwrap(), stand);
    }

    #[test]
    fn the_router_names_what_is_missing() {
        let base = "schema = 1\n[station]\nssid = \"Made Up Lab Net\"\n[station_fixture]\n";
        let full = "kind = \"open-wrt\"\nssh_target = \"made-up-router.lab\"\nwireless_interface = \"phy7-ap3\"\nradio = \"radio9\"\nap_section = \"lab_ap\"\nchannel = 6\n";
        Stand::parse(&format!("{base}{full}"))
            .unwrap()
            .router()
            .unwrap();
        for key in [
            "kind",
            "ssh_target",
            "wireless_interface",
            "radio",
            "ap_section",
            "channel",
        ] {
            let text = full
                .lines()
                .filter(|line| !line.starts_with(key))
                .collect::<Vec<_>>()
                .join("\n");
            let error = Stand::parse(&format!("{base}{text}\n"))
                .unwrap()
                .router()
                .unwrap_err();
            assert!(
                error.contains(&format!("[station_fixture].{key}")),
                "{error}"
            );
        }
        let error = Stand::parse(&format!("schema = 1\n[station_fixture]\n{full}"))
            .unwrap()
            .router()
            .unwrap_err();
        assert!(error.contains("[station].ssid"), "{error}");
        let error = Stand::parse(&format!(
            "{base}{}",
            full.replace("open-wrt", "local-linux")
        ))
        .unwrap()
        .router()
        .unwrap_err();
        assert!(error.contains("is not `open-wrt`"), "{error}");
        // Without a router the stand still serves the boards.
        assert!(Stand::parse("schema = 1\n").unwrap().router().is_err());
    }

    #[test]
    fn a_type_error_does_not_quote_the_value() {
        for text in [
            "schema = 1\n[station_fixture]\nchannel = \"made-up-router\"\n",
            "schema = 1\n[station]\nssid = [\"Made Up Lab Net\"]\n",
            "schema = 1\n[station_fixture]\nssh_target = 7\n",
        ] {
            let error = Stand::parse(text).unwrap_err();
            for secret in SECRETS {
                assert!(!error.contains(secret), "{error}");
            }
            assert!(error.starts_with("line "), "{error}");
        }
    }

    #[test]
    fn reads_hubs_and_boards_and_ignores_the_rest() {
        let stand = Stand::parse(STAND).unwrap();
        assert_eq!(stand.hubs.len(), 1);
        assert_eq!(stand.hubs[0].usb3.as_deref(), Some("4-1"));
        let board = stand.board("s31-a").unwrap();
        assert_eq!(board.mac, "02:00:00:00:01:D0");
        assert_eq!(stand.board("02:00:00:00:01:d0"), Some(board));
        assert_eq!(stand.board_by_mac("02:00:00:00:01:D0"), Some(board));
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
    fn only_a_file_of_mode_0600_is_read() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stand.toml");
        std::fs::write(&path, STAND).unwrap();
        for mode in [0o644, 0o640, 0o604, 0o400, 0o700] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let error = Stand::load(&path).unwrap_err().to_string();
            assert!(
                error.contains(&format!("mode {mode:04o}")) && error.contains("0600"),
                "{error}"
            );
            for secret in SECRETS {
                assert!(!error.contains(secret), "{error}");
            }
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(Stand::load(&path).unwrap(), Stand::parse(STAND).unwrap());
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
