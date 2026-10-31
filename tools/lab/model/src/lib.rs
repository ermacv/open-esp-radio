//! The owner's stand file (README.md): what no tool can discover about the
//! stand. That is its identity and air policy, its hubs' port policy, its
//! boards (name, MAC, chip, radios, roles, reset ladder), the lab router
//! (`[router]`, equipment of the LAB service), the host's fixtures (its
//! access point and station, the Bluetooth adapter, the legacy BSS country)
//! and the air observer.
//! Which hub port powers a board is found on the host
//! (`oer_lab_router::hub`), not written here.
//!
//! The file holds no credentials: every access point and station of the
//! stand is managed, and a run generates the SSID and passphrase it sets on
//! both. An SSID or passphrase a file still holds is never deserialized.
//! Each consumer reads only its keys; a missing key fails at the consumer
//! that needs it and names the key. A file whose mode is not 0600 is
//! refused unread, and a parse error names its line and never a value. SSH
//! targets and interface names are [`Secret`]s: their `Debug` shows nothing
//! of them. [`paths`] places the stand's state; [`convert`] rewrites a
//! schema 1 file.
//!
//! What is known of a board and the stand's equipment apart from the
//! hardware lives here too, for the LAB service's clients: reset paths and
//! the ROM's reset lines ([`rom`]), the reference peers' lines ([`peer_line`]),
//! `uhubctl`'s report ([`hubs`]) and the equipment reports ([`equipment`]).

#![forbid(unsafe_code)]

pub mod convert;
pub mod device_id;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub use crate::device_id::DeviceId;

pub mod equipment;
pub mod hubs;
pub mod paths;
pub mod peer_line;
pub mod rom;

/// The crate's error type.
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

/// The schema this tool reads. Schema 1 described the lab router as a
/// station fixture with the run's channel; [`convert`] rewrites it.
pub const SCHEMA: u32 = 2;

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
    /// Ports never switched off.
    #[serde(default)]
    pub protected: Vec<u32>,
    /// When not empty, the only ports whose power may be switched.
    #[serde(default)]
    pub switchable: Vec<u32>,
    /// Ports that report `off` but keep their power: never switched.
    #[serde(default, rename = "keeps-power")]
    pub keeps_power: Vec<u32>,
    /// The hub's touch button of each port, counted from the top of the
    /// stand: port → button.
    #[serde(default)]
    pub buttons: BTreeMap<String, u8>,
}

impl Hub {
    pub fn is_protected(&self, port: u32) -> bool {
        self.protected.contains(&port)
    }

    /// Whether the owner allows switching `port`: listed as switchable, or,
    /// without a list, any port neither protected nor keeping its power.
    pub fn is_switchable(&self, port: u32) -> bool {
        if self.is_protected(port) || self.keeps_power.contains(&port) {
            return false;
        }
        self.switchable.is_empty() || self.switchable.contains(&port)
    }

    /// The touch button of `port`, if the hub has one there.
    pub fn button(&self, port: u32) -> Option<u8> {
        self.buttons.get(&port.to_string()).copied()
    }
}

/// Where an older stand file said a board's USB cable plugs in. Only read
/// to warn that it is ignored: the port is found on the host.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct PortRef {
    pub hub: String,
    pub port: u32,
}

/// A radio a board carries.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Radio {
    #[serde(rename = "wifi-2g4")]
    Wifi2g4,
    #[serde(rename = "wifi-5g")]
    Wifi5g,
    Ble,
    Ieee802154,
}

impl Radio {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wifi2g4 => "wifi-2g4",
            Self::Wifi5g => "wifi-5g",
            Self::Ble => "ble",
            Self::Ieee802154 => "ieee802154",
        }
    }

    /// Whether a chip with `properties` has this radio.
    fn on(self, properties: &oer_espressif_facts::Properties) -> bool {
        use oer_espressif_facts::{BluetoothMode, WifiBand};
        match self {
            Self::Wifi2g4 => properties.wifi_bands.contains(&WifiBand::Band2g4),
            Self::Wifi5g => properties.wifi_bands.contains(&WifiBand::Band5g),
            Self::Ble => properties.bluetooth.contains(&BluetoothMode::Le),
            Self::Ieee802154 => properties.ieee802154,
        }
    }
}

/// A role a run gives a board: what it flashes on it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BoardRole {
    /// Our image under test.
    Dut,
    /// A vendor (ESP-IDF) firmware the device under test works against.
    Peer,
}

impl BoardRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dut => "dut",
            Self::Peer => "peer",
        }
    }
}

/// One way to reset a board.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResetStep {
    /// RTS of the USB Serial/JTAG port.
    UsbJtagRts,
    /// A reset through the USB Serial/JTAG port's JTAG.
    Jtag,
    /// A power cycle of the board's hub port.
    Power,
}

impl ResetStep {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UsbJtagRts => "usb-jtag-rts",
            Self::Jtag => "jtag",
            Self::Power => "power",
        }
    }
}

/// A board as every stand component names it: its id, its MAC and its chip.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BoardRef {
    pub id: String,
    /// Its USB serial number as a board id.
    pub mac: DeviceId,
    pub chip: String,
}

impl BoardRef {
    /// `id (chip)`.
    pub fn label(&self) -> String {
        format!("{} ({})", self.id, self.chip)
    }
}

/// A board of the stand.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Board {
    pub id: String,
    /// The USB serial number of its USB Serial/JTAG: the chip's MAC.
    #[serde(rename = "usb-serial")]
    pub mac: String,
    pub chip: String,
    /// The radios the board carries; a subset of its chip's.
    #[serde(default)]
    pub radios: Vec<Radio>,
    /// The roles a run may give it; a board without one takes none.
    #[serde(default)]
    pub roles: Vec<BoardRole>,
    /// How the stand resets it, in the order it tries them.
    #[serde(default)]
    pub reset: Vec<ResetStep>,
    /// An image the board's startup flash holds, when it needs one.
    #[serde(default, rename = "startup-artifact")]
    pub startup_artifact: Option<PathBuf>,
    /// Ignored with a warning: see [`PortRef`].
    #[serde(default)]
    pub port: Option<PortRef>,
}

impl Board {
    pub fn has_role(&self, role: BoardRole) -> bool {
        self.roles.contains(&role)
    }

    /// Whether `serial` is this board's USB serial number, in any of the
    /// forms [`DeviceId::parse`] reads.
    pub fn is_serial(&self, serial: &str) -> bool {
        match (DeviceId::parse(&self.mac), DeviceId::parse(serial)) {
            (Ok(own), Ok(other)) => own == other,
            _ => self.mac.eq_ignore_ascii_case(serial),
        }
    }

    /// The board's id: its USB serial number as a [`DeviceId`].
    pub fn device_id(&self) -> Result<DeviceId> {
        DeviceId::parse(&self.mac).map_err(|cause| format!("board `{}`: {cause}", self.id).into())
    }

    /// The board as stand components name it.
    pub fn reference(&self) -> Result<BoardRef> {
        Ok(BoardRef {
            id: self.id.clone(),
            mac: self.device_id()?,
            chip: self.chip.clone(),
        })
    }

    /// Check the board's chip and radios against the chip profiles `chips`.
    pub fn validate_chip(&self, chips: &[oer_chip_profile::Profile]) -> Result<()> {
        let name = &self.id;
        let profile = chips
            .iter()
            .find(|profile| profile.facts.id == self.chip)
            .ok_or_else(|| {
                format!(
                    "board `{name}`: chip `{}` has no profile (portable/{}/platform/chip.toml)",
                    self.chip, self.chip
                )
            })?;
        match self
            .radios
            .iter()
            .find(|radio| !radio.on(&profile.facts.properties))
        {
            Some(radio) => Err(format!(
                "board `{name}`: an {} has no {} radio",
                self.chip,
                radio.as_str()
            )
            .into()),
            None => Ok(()),
        }
    }

    /// The warning for an older stand file's `port`, ignored for the port
    /// `detected` on the host, if the file still has one.
    pub fn ignored_port(&self, detected: &str) -> Option<String> {
        self.port.as_ref().map(|port| {
            format!(
                "stand file: board {} port = {}:{} is ignored, detected {detected}",
                self.id, port.hub, port.port
            )
        })
    }
}

/// A value of the stand file that is never shown: its `Debug` is
/// `<hidden>`, and it has no `Display`.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
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

/// `[stand]`: the stand's identity and how runs share its air.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Identity {
    pub id: String,
    pub air: AirPolicy,
}

/// How runs share the air.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum AirPolicy {
    /// A run that uses the air holds all of it while it executes.
    Exclusive,
}

/// `[station]`: how the stand's station takes its IPv4 address. Its SSID
/// and passphrase are a run's, never the file's.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Station {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv4: Option<toml::Value>,
}

/// `[access_point]`: the host's access point a run's station joins, without
/// its SSID and passphrase, which the run generates. The channel and width
/// a run uses are the run's, chosen among these.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AccessPoint {
    /// The channels the stand may use for it.
    pub channels: Vec<u8>,
    /// The channel widths the stand may use for it.
    pub widths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_limit: Option<u8>,
    pub target_address: String,
    pub client_address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_client_address: Option<String>,
}

/// `[router]`: the lab's OpenWrt router, equipment the LAB service owns
/// and reaches over SSH. It describes the router, not a run: the channel
/// and mode a run applies are the run's, chosen among `channels` and
/// `phys`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Router {
    /// Its firmware; `open-wrt` is the one the service drives.
    pub kind: String,
    pub ssh_target: Secret,
    /// The UCI `wifi-device` section of its radio.
    pub radio: String,
    /// The UCI `wifi-iface` section of the stand's AP.
    pub ap_section: String,
    pub wireless_interface: Secret,
    /// Its wired port toward this host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingress_interface: Option<Secret>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor_interface: Option<Secret>,
    /// The AP modes its radio supports, as its owner reads them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phys: Vec<String>,
    /// The channels the stand may use on it.
    pub channels: Vec<u8>,
    /// The laptop's monitor radio observes independently of the router.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub independent_laptop_monitor: Option<bool>,
    /// A protected AP: its identity, security and channel are verified,
    /// never applied or restored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    /// Its address on the wired network this host reaches it by: the
    /// gateway of this host's route to the station network, which a load
    /// run checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lan_address: Option<std::net::Ipv4Addr>,
}

/// `[local_station]`: a station on this host: its interface, country,
/// address and coexistence.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct LocalStation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<Secret>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coexistence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
}

/// `[air_observer]`: the idle OpenWrt radio that observes the air.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AirObserver {
    pub ssh_target: Secret,
    pub phy: String,
    pub interface: Secret,
}

/// `[bluetooth]`: the host's Bluetooth adapter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Bluetooth {
    pub adapter: String,
}

/// `[legacy_bss]`: the regulatory domain of the laptop's non-ERP BSS.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LegacyBss {
    pub country: String,
}

/// The stand file, as its consumers read it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Stand {
    pub schema: u32,
    #[serde(default)]
    pub stand: Option<Identity>,
    #[serde(default, rename = "hub")]
    pub hubs: Vec<Hub>,
    #[serde(default, rename = "board")]
    pub boards: Vec<Board>,
    #[serde(default)]
    pub station: Station,
    #[serde(default)]
    pub access_point: Option<AccessPoint>,
    #[serde(default)]
    pub router: Option<Router>,
    #[serde(default)]
    pub local_station: Option<LocalStation>,
    #[serde(default)]
    pub air_observer: Option<AirObserver>,
    #[serde(default)]
    pub bluetooth: Option<Bluetooth>,
    #[serde(default)]
    pub legacy_bss: Option<LegacyBss>,
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
                "stand file {} has mode {mode:04o}: it holds the stand's credentials and must be \
                 {MODE:04o} (`chmod 600 {}`)",
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
        // The schema decides how the rest reads.
        #[derive(Deserialize)]
        struct Schema {
            schema: u32,
        }
        let schema = toml::from_str::<Schema>(text)
            .map(|schema| schema.schema)
            .unwrap_or(SCHEMA);
        if schema == 1 {
            return Err(
                "schema 1 described the lab router as a station fixture; `cargo stand \
                 stand-file convert` rewrites it to schema 2"
                    .to_owned(),
            );
        }
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

    /// The board named `selector`: its id, or its MAC in any case.
    pub fn board(&self, selector: &str) -> Option<&Board> {
        self.boards
            .iter()
            .find(|board| board.id == selector || board.mac.eq_ignore_ascii_case(selector))
    }

    /// The board whose MAC is `mac`.
    pub fn board_by_mac(&self, mac: &str) -> Option<&Board> {
        self.boards.iter().find(|board| board.is_serial(mac))
    }

    pub fn hub(&self, id: &str) -> Option<&Hub> {
        self.hubs.iter().find(|hub| hub.id == id)
    }

    /// `[stand]`, which a HIL run needs.
    pub fn identity(&self) -> Result<&Identity> {
        self.stand
            .as_ref()
            .ok_or_else(|| "the stand file has no [stand] (id, air), which a HIL run needs".into())
    }

    /// The host's fixture sections as tables, for the fixture owner that
    /// types them: `station`, `access_point`, `local_station`,
    /// `air_observer`, `bluetooth` and `legacy_bss`, each only with the keys
    /// this model reads, so no credential the file still holds is among
    /// them. The router is the LAB service's, not a fixture.
    pub fn fixtures(&self) -> Result<toml::Table> {
        let mut table = toml::Table::new();
        let mut put = |name: &str, value: Option<toml::Value>| {
            if let Some(value) = value {
                table.insert(name.to_owned(), value);
            }
        };
        put("station", Some(toml::Value::try_from(&self.station)?));
        put(
            "access_point",
            self.access_point
                .as_ref()
                .map(toml::Value::try_from)
                .transpose()?,
        );
        put(
            "local_station",
            self.local_station
                .as_ref()
                .map(toml::Value::try_from)
                .transpose()?,
        );
        put(
            "air_observer",
            self.air_observer
                .as_ref()
                .map(toml::Value::try_from)
                .transpose()?,
        );
        put(
            "bluetooth",
            self.bluetooth
                .as_ref()
                .map(toml::Value::try_from)
                .transpose()?,
        );
        put(
            "legacy_bss",
            self.legacy_bss
                .as_ref()
                .map(toml::Value::try_from)
                .transpose()?,
        );
        Ok(table)
    }

    /// Check every board's chip and radios against the chip profiles
    /// `chips`, as the stand's doctor does.
    pub fn validate_chips(&self, chips: &[oer_chip_profile::Profile]) -> Result<()> {
        self.boards
            .iter()
            .try_for_each(|board| board.validate_chip(chips))
    }

    /// Check what the file's boards and hubs promise: distinct hub ids and
    /// locations, ports counted from 1, no port both protected and
    /// switchable, distinct buttons; distinct board ids and MACs, valid
    /// identifiers, no repeated radio, role or reset step.
    pub fn validate(&self) -> Result<()> {
        if let Some(identity) = &self.stand {
            identifier("stand.id", &identity.id)?;
        }
        let mut ids = BTreeSet::new();
        let mut locations = BTreeSet::new();
        let mut buttons = BTreeSet::new();
        for hub in &self.hubs {
            identifier("hub.id", &hub.id)?;
            if !ids.insert(hub.id.as_str()) {
                return Err(format!("hub `{}` is described twice", hub.id).into());
            }
            for location in std::iter::once(&hub.usb2).chain(&hub.usb3) {
                if location.is_empty() || !locations.insert(location.as_str()) {
                    return Err(format!(
                        "hub `{}`: a USB location is empty or another hub's",
                        hub.id
                    )
                    .into());
                }
            }
            if hub
                .protected
                .iter()
                .chain(&hub.switchable)
                .any(|&port| port == 0)
            {
                return Err(format!("hub `{}`: ports count from 1", hub.id).into());
            }
            if let Some(port) = hub
                .protected
                .iter()
                .find(|port| hub.switchable.contains(port))
            {
                return Err(format!(
                    "hub `{}`: port {port} is both protected and switchable",
                    hub.id
                )
                .into());
            }
            for (port, &button) in &hub.buttons {
                if !port.parse::<u32>().is_ok_and(|port| port > 0) {
                    return Err(
                        format!("hub `{}`: button port `{port}` is not a port", hub.id).into(),
                    );
                }
                if button == 0 || !buttons.insert(button) {
                    return Err(format!(
                        "hub `{}`: button {button} is zero or another port's",
                        hub.id
                    )
                    .into());
                }
            }
        }
        let mut ids = BTreeSet::new();
        let mut macs = BTreeSet::new();
        for board in &self.boards {
            identifier("board.id", &board.id)?;
            if !ids.insert(board.id.as_str()) {
                return Err(format!("board `{}` is described twice", board.id).into());
            }
            if !macs.insert(board.device_id()?) {
                return Err(format!("board `{}`: its MAC is another board's", board.id).into());
            }
            identifier(&format!("board `{}` chip", board.id), &board.chip)?;
            distinct(&board.id, "radios", &board.radios)?;
            distinct(&board.id, "roles", &board.roles)?;
            distinct(&board.id, "reset", &board.reset)?;
        }
        Ok(())
    }

    /// The board `query` names: its id, its chip when it is the only board
    /// of that chip, or its MAC. Every stand command and component turns a
    /// board name into a board this way.
    pub fn resolve(&self, query: &str) -> Result<&Board> {
        if let Some(board) = self.boards.iter().find(|board| board.id == query) {
            return Ok(board);
        }
        let of_chip = self
            .boards
            .iter()
            .filter(|board| board.chip == query)
            .collect::<Vec<_>>();
        match of_chip.as_slice() {
            [board] => return Ok(board),
            [] => {}
            several => {
                return Err(format!(
                    "the stand file has several {query} boards ({}); name one of them",
                    ids_of(several)
                )
                .into());
            }
        }
        match DeviceId::parse(query) {
            Ok(mac) => self.board_by_mac(&mac.to_string()).ok_or_else(|| {
                format!("no board of the stand file has the MAC {mac}; add it to the stand file")
                    .into()
            }),
            Err(_) => Err(format!(
                "board `{query}` is neither a board id, a chip with one board nor a MAC of the \
                 stand file"
            )
            .into()),
        }
    }

    /// The board with `mac`'s label, `id (chip)`, or the MAC itself.
    pub fn label(&self, mac: &str) -> String {
        self.board_by_mac(mac).map_or_else(
            || mac.to_owned(),
            |board| format!("{} ({})", board.id, board.chip),
        )
    }

    /// The board of `chip` a run gives `role`: `chosen` when the run names
    /// one, otherwise the only such board other than `excluding` (the board
    /// another role of the run already took).
    pub fn select_board(
        &self,
        chip: &str,
        role: BoardRole,
        chosen: Option<&str>,
        excluding: Option<&str>,
    ) -> Result<&Board> {
        if let Some(chosen) = chosen {
            let board = self
                .boards
                .iter()
                .find(|board| board.id == chosen)
                .ok_or_else(|| format!("board `{chosen}` is not in the stand file"))?;
            if board.chip != chip || !board.has_role(role) {
                return Err(format!(
                    "board `{chosen}` is an {} for {}; this run needs an {chip} for {}",
                    board.chip,
                    board
                        .roles
                        .iter()
                        .map(|role| role.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    role.as_str()
                )
                .into());
            }
            if Some(chosen) == excluding {
                return Err(
                    format!("board `{chosen}` already has another role in this run").into(),
                );
            }
            return Ok(board);
        }
        let candidates = self
            .boards
            .iter()
            .filter(|board| {
                board.chip == chip && board.has_role(role) && Some(board.id.as_str()) != excluding
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [board] => Ok(board),
            [] => Err(format!(
                "the stand file has no {chip} board for {}{}",
                role.as_str(),
                excluding.map_or(String::new(), |board| format!(" besides `{board}`"))
            )
            .into()),
            several => Err(format!(
                "several {chip} boards can be the {} ({}); name one",
                role.as_str(),
                ids_of(several)
            )
            .into()),
        }
    }
}

fn ids_of(boards: &[&Board]) -> String {
    boards
        .iter()
        .map(|board| board.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn identifier(field: &str, value: &str) -> Result<()> {
    if oer_espressif_facts::valid_identifier(value) {
        Ok(())
    } else {
        Err(format!(
            "{field} `{value}` must start with a lowercase letter and hold only lowercase \
             letters, digits and hyphens"
        )
        .into())
    }
}

fn distinct<T: Ord + Copy>(board: &str, field: &str, values: &[T]) -> Result<()> {
    if values.iter().copied().collect::<BTreeSet<_>>().len() != values.len() {
        return Err(format!("board `{board}`: {field} repeats a value").into());
    }
    Ok(())
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
schema = 2

[stand]
id = "made-up-lab"
air = "exclusive"
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
radios = ["wifi-2g4", "ble"]
roles = ["dut"]
port = { hub = "top", port = 4 }
reset = ["power"]

[[board]]
id = "behind-hub"
usb-serial = "02:00:00:00:EE:FF"
chip = "esp32s31"
roles = ["dut"]
port = { hub = "top", port = 3 }

[[board]]
id = "c5-a"
usb-serial = "02:00:00:00:C5:01"
chip = "esp32c5"
roles = ["peer"]

[station]
ssid = "Made Up Lab Net"
passphrase = "made-up-passphrase"

[router]
kind = "open-wrt"
ssh_target = "made-up-router.lab"
wireless_interface = "phy7-ap3"
radio = "radio9"
ap_section = "lab_ap"
channels = [6]
ingress_interface = "made-up-ingress"

[access_point]
ssid = "made-up-ap-ssid"
passphrase = "made-up-ap-passphrase"
channels = [6]
widths = ["ht20"]
target_address = "192.168.77.1/24"
client_address = "192.168.77.2/24"
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
    fn reads_the_fixture_and_nothing_secret_is_kept_or_shown() {
        let stand = Stand::parse(STAND).unwrap();
        let fixtures = stand.fixtures().unwrap();
        let shown = format!("{stand:?}");
        for secret in SECRETS {
            assert!(!shown.contains(secret), "{secret}: {shown}");
        }
        // No SSID or passphrase is deserialized: the parsed stand is the same
        // without them, and the fixture tables hold none.
        let without = STAND
            .lines()
            .filter(|line| !line.starts_with("ssid") && !line.starts_with("passphrase"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(Stand::parse(&without).unwrap(), stand);
        let tables = fixtures.to_string();
        for credential in [
            "Made Up Lab Net",
            "made-up-passphrase",
            "made-up-ap-ssid",
            "made-up-ap-passphrase",
        ] {
            assert!(!tables.contains(credential), "{credential}");
        }
        assert_eq!(
            fixtures["access_point"]["channels"]
                .as_array()
                .and_then(|channels| channels.first())
                .and_then(toml::Value::as_integer),
            Some(6)
        );
        assert!(
            !tables.contains("made-up-router"),
            "the router is no fixture"
        );
    }

    #[test]
    fn a_type_error_does_not_quote_the_value() {
        for text in [
            "schema = 2\n[router]\nchannels = \"made-up-router\"\n",
            "schema = 2\n[air_observer]\nssh_target = [\"made-up-router\"]\nphy = \"p\"\ninterface = \"i\"\n",
            "schema = 2\n[router]\nssh_target = 7\n",
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
        assert_eq!(board.roles, [BoardRole::Dut]);
        assert_eq!(board.reset, [ResetStep::Power]);
        assert_eq!(stand.board("02:00:00:00:01:d0"), Some(board));
        assert_eq!(stand.board_by_mac("02:00:00:00:01:D0"), Some(board));
        assert_eq!(stand.board("nope"), None);
    }

    #[test]
    fn an_older_board_port_is_only_a_warning() {
        let stand = Stand::parse(STAND).unwrap();
        let warning = stand
            .board("s31-a")
            .unwrap()
            .ignored_port("3-8.3.4 port 4")
            .unwrap();
        assert_eq!(
            warning,
            "stand file: board s31-a port = top:4 is ignored, detected 3-8.3.4 port 4"
        );
        let hub = stand.hub("top").unwrap();
        assert_eq!(
            (hub.protected.as_slice(), hub.keeps_power.as_slice()),
            (&[3][..], &[][..])
        );
    }

    #[test]
    fn a_parse_error_does_not_quote_the_file() {
        let error = Stand::parse("schema = 2\n[station]\npassword = hunter2\n").unwrap_err();
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
    fn a_board_resolves_by_id_chip_or_mac_and_is_selected_by_role() {
        let stand = Stand::parse(STAND).unwrap();
        stand.validate().unwrap();
        assert_eq!(stand.resolve("c5-a").unwrap().id, "c5-a");
        assert_eq!(stand.resolve("esp32c5").unwrap().id, "c5-a");
        assert_eq!(stand.resolve("02:00:00:00:c5:01").unwrap().id, "c5-a");
        let error = stand.resolve("esp32s31").unwrap_err().to_string();
        assert!(error.contains("several esp32s31 boards"), "{error}");
        assert_eq!(
            stand
                .select_board("esp32c5", BoardRole::Peer, None, None)
                .unwrap()
                .id,
            "c5-a"
        );
        assert!(
            stand
                .select_board("esp32c5", BoardRole::Dut, None, None)
                .is_err()
        );
        assert_eq!(
            stand
                .select_board("esp32s31", BoardRole::Dut, Some("s31-a"), None)
                .unwrap()
                .id,
            "s31-a"
        );
        assert!(
            stand
                .select_board("esp32s31", BoardRole::Dut, Some("s31-a"), Some("s31-a"))
                .is_err()
        );
        assert_eq!(stand.label("02:00:00:00:01:d0"), "s31-a (esp32s31)");
        assert_eq!(stand.identity().unwrap().air, AirPolicy::Exclusive);
        assert!(Stand::parse("schema = 2\n").unwrap().identity().is_err());
    }

    #[test]
    fn validation_refuses_a_twice_described_board_or_hub_policy_conflict() {
        let twice = format!(
            "{STAND}\n[[board]]\nid = \"c5-a\"\nusb-serial = \"02:00:00:00:C5:02\"\nchip = \"esp32c5\"\n"
        );
        assert!(Stand::parse(&twice).unwrap().validate().is_err());
        let conflict = STAND.replace("switchable = [1, 4]", "switchable = [3, 4]");
        assert!(Stand::parse(&conflict).unwrap().validate().is_err());
        let hub = Stand::parse(STAND).unwrap().hubs[0].clone();
        assert!(hub.is_switchable(4) && !hub.is_switchable(3) && !hub.is_switchable(2));
        assert_eq!(hub.button(1), Some(1));
    }

    #[test]
    fn another_schema_is_refused() {
        assert!(
            Stand::parse("schema = 3\n")
                .unwrap_err()
                .contains("schema 3")
        );
    }
}
