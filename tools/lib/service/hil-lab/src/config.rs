//! Typed, host-local description of the physical HIL cell.

use std::{
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

use crate::wifi::{NetworkIpv4Configuration, WifiChannelWidth};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::Result;
use oer_tool_data_hil_scenario_catalog::link::{PhyExpectation, WifiLabUse};

/// The stand file's view a run uses: the pool and the fixture sections, with
/// the run's device under test taken from the pool.
#[derive(Clone)]
pub struct LabConfig {
    path: PathBuf,
    cell_id: String,
    /// The chip of `dut`.
    chip: String,
    /// The stand file's pool, from which [`Self::peer`] takes a
    /// peer board.
    stand: oer_tool_data_stand_file::Stand,
    /// The boards the run named.
    choice: BoardChoice,
    /// The chip of the run's peer images ([`Self::with_peer_images`]); none
    /// when the run uses no peer.
    peer_chip: Option<String>,
    /// The device under test of `chip`.
    pub dut: DeviceConfig,
    /// The stand's Bluetooth adapter (`[bluetooth] adapter`): stand
    /// inventory each run's lab provenance records; no current scenario
    /// drives it.
    pub bluetooth_adapter: Option<BluetoothAdapter>,
    pub station: StationConfig,
    pub access_point: AccessPointConfig,
    pub station_fixture: StationFixtureConfig,
    pub air_observer: Option<AirObserverConfig>,
    pub legacy_bss: Option<LegacyBssConfig>,
}

/// The laptop radio may host a non-ERP (802.11b) BSS on the laboratory AP's
/// channel. Its regulatory domain is explicit because the laboratory AP
/// configuration carries none.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyBssConfig {
    pub country: String,
}

/// The fixture sections of the stand file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFixtures {
    bluetooth: Option<RawBluetoothConfig>,
    station: RawStationConfig,
    access_point: RawAccessPointConfig,
    station_fixture: RawStationFixtureConfig,
    air_observer: Option<AirObserverConfig>,
    legacy_bss: Option<LegacyBssConfig>,
}

/// The boards a run names: needed only where the pool has several
/// candidates of a chip and role, until the stand's scheduler assigns them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BoardChoice {
    /// The device under test (`--board`).
    pub dut: Option<String>,
    /// The peer board (`--peer-board`).
    pub peer: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBluetoothConfig {
    adapter: String,
}

/// A Linux Bluetooth adapter, `hciN` in canonical decimal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BluetoothAdapter(pub u16);

impl std::str::FromStr for BluetoothAdapter {
    type Err = String;
    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let invalid = || "adapter must be hci0 through hci65534 (canonical decimal)".to_owned();
        let number = value.strip_prefix("hci").ok_or_else(invalid)?;
        let index = number.parse::<u16>().map_err(|_| invalid())?;
        if index == u16::MAX || number != index.to_string() {
            return Err(invalid());
        }
        Ok(Self(index))
    }
}

impl std::fmt::Display for BluetoothAdapter {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(output, "hci{}", self.0)
    }
}

/// A peer board of a run: its stand-file id and its serial port. Each
/// scenario that uses a peer names the catalog image it must carry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerBoardConfig {
    pub id: String,
    /// The board's MAC, which names it in claims, locks and the journal.
    pub mac: String,
    pub chip: String,
    /// The peer's port, or why its board has none: a board that is not
    /// attached fails only the runs that use the peer.
    port: std::result::Result<PathBuf, String>,
}

impl PeerBoardConfig {
    pub fn new(id: String, mac: String, chip: String, serial: PathBuf) -> Self {
        Self {
            id,
            mac,
            chip,
            port: Ok(serial),
        }
    }

    /// The peer's serial port; an error when its board is not attached.
    pub fn serial(&self) -> Result<PathBuf> {
        self.port
            .clone()
            .map_err(|error| format!("peer board: {error}").into())
    }
}

#[derive(Clone, Debug)]
pub struct DeviceConfig {
    pub id: String,
    /// The board's MAC, which names it in claims, locks and the journal and
    /// outlives its port name, which a reset can change.
    pub mac: String,
    pub serial: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStationConfig {
    ipv4: RawIpv4Config,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
enum RawIpv4Config {
    Dhcp,
    Static {
        address: String,
        gateway: Option<Ipv4Addr>,
    },
}

#[derive(Clone)]
pub struct StationConfig {
    ssid: Zeroizing<String>,
    passphrase: Zeroizing<String>,
    ipv4: NetworkIpv4Configuration,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAccessPointConfig {
    channel: u8,
    channel_width: WifiChannelWidth,
    #[serde(default = "default_ap_client_limit")]
    client_limit: u8,
    target_address: String,
    client_address: String,
    secondary_client_address: Option<String>,
}

const fn default_ap_client_limit() -> u8 {
    4
}

#[derive(Clone)]
pub struct AccessPointConfig {
    ssid: Zeroizing<String>,
    passphrase: Zeroizing<String>,
    channel: u8,
    channel_width: WifiChannelWidth,
    client_limit: u8,
    target_address: Ipv4Addr,
    client_address: Ipv4Addr,
    secondary_client_address: Option<Ipv4Addr>,
    prefix_length: u8,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum RawStationFixtureConfig {
    LocalLinux {
        interface: String,
        phys: Vec<PhyExpectation>,
        country: String,
        channel: u8,
        address: String,
        #[serde(default)]
        coexistence: Coexistence,
    },
    OpenWrt {
        radio: String,
        ap_section: String,
        channel: u8,
        ssh_target: String,
        wireless_interface: String,
        ingress_interface: String,
        monitor_interface: Option<String>,
        phys: Vec<PhyExpectation>,
        #[serde(default)]
        independent_laptop_monitor: bool,
        #[serde(default)]
        read_only: bool,
    },
    External {
        phys: Vec<PhyExpectation>,
    },
}

#[derive(Clone, Debug)]
pub enum StationFixtureConfig {
    LocalLinux(LocalLinuxConfig),
    OpenWrt(OpenWrtConfig),
    External(ExternalConfig),
}

#[derive(Clone, Debug)]
pub struct LocalLinuxConfig {
    pub interface: String,
    pub phys: Vec<PhyExpectation>,
    pub country: String,
    pub channel: u8,
    pub ht40_above: bool,
    pub address: Ipv4Addr,
    pub prefix_length: u8,
    pub coexistence: Coexistence,
}

#[derive(Clone, Copy, Debug, Default, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Coexistence {
    #[default]
    Respect,
    ForceHt40,
}

#[derive(Clone, Debug)]
pub struct OpenWrtConfig {
    pub ht40_above: bool,
    pub radio: String,
    pub ap_section: String,
    pub channel: u8,
    pub ssh_target: String,
    pub wireless_interface: String,
    pub ingress_interface: String,
    pub monitor_interface: Option<String>,
    pub phys: Vec<PhyExpectation>,
    pub independent_laptop_monitor: bool,
    pub read_only: bool,
}

/// Dedicated idle OpenWrt radio used for independent over-the-air evidence.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AirObserverConfig {
    pub ssh_target: String,
    pub phy: String,
    pub interface: String,
}

#[derive(Clone, Debug)]
pub struct ExternalConfig {
    pub phys: Vec<PhyExpectation>,
}

/// The serial port of `board`, its `/dev/serial/by-id` link among the
/// attached ports.
fn attached_port(board: &oer_tool_data_stand_file::Board) -> Result<PathBuf> {
    oer_tool_service_devices::discovery::port_of(&board.device_id()?)
        .map_err(|error| format!("board `{}`: {error}", board.id).into())
}

/// The chip a peer catalog image (`hil/peers/*/firmware.toml`) targets.
pub fn peer_image_chip(root: &Path, image: &str) -> Result<String> {
    let mut manifests = fs::read_dir(root.join("hil/peers"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path().join("firmware.toml")))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    manifests.sort();
    for manifest in manifests {
        let table: toml::Table = toml::from_str(&fs::read_to_string(&manifest)?)
            .map_err(|error| format!("invalid {}: {error}", manifest.display()))?;
        if table.get("image").and_then(toml::Value::as_str) == Some(image) {
            return table
                .get("chip")
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("{} names no chip", manifest.display()).into());
        }
    }
    Err(format!("no peer catalog image `{image}` in hil/peers").into())
}

impl LabConfig {
    /// The host's stand file, which every checkout reads.
    pub fn default_path() -> Result<PathBuf> {
        Ok(oer_tool_data_stand_file::paths::stand_file()?)
    }

    /// The stand file at `path` with `chip`'s device under test, the one
    /// `choice` names or the only one of the pool.
    pub fn load(path: &Path, chip: &str, choice: &BoardChoice) -> Result<Self> {
        Self::load_resolving(path, chip, choice, &attached_port)
    }

    /// Load with `resolve` mapping a board to its serial port.
    pub(crate) fn load_resolving(
        path: &Path,
        chip: &str,
        choice: &BoardChoice,
        resolve: &dyn Fn(&oer_tool_data_stand_file::Board) -> Result<PathBuf>,
    ) -> Result<Self> {
        let stand = oer_tool_data_stand_file::Stand::load(path)?;
        let root = oer_tool_base_process::built_root();
        let chips = oer_tool_data_chip_profile::Profile::all(&root)?;
        // The fixture sections hold no credentials: the run generates them.
        let raw: RawFixtures =
            toml::Value::Table(stand.fixtures()?)
                .try_into()
                .map_err(|error| {
                    format!("invalid fixture sections in `{}`: {error}", path.display())
                })?;
        if let Some(observer) = &raw.air_observer {
            for (name, value) in [
                ("air_observer.ssh_target", &observer.ssh_target),
                ("air_observer.phy", &observer.phy),
                ("air_observer.interface", &observer.interface),
            ] {
                validate_shell_token(name, value)?;
            }
            if !matches!(raw.station_fixture, RawStationFixtureConfig::OpenWrt { .. }) {
                return Err("independent OpenWrt observer requires a managed OpenWrt AP".into());
            }
        }
        let board = stand
            .select_board(
                chip,
                oer_tool_data_stand_file::BoardRole::Dut,
                choice.dut.as_deref(),
                None,
            )
            .map_err(|error| format!("{error} (--board)"))?;
        board
            .validate_chip(&chips)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let device_serial = resolve(board)?;
        let device_mac = board.device_id()?.to_string();
        let device_id = board.id.clone();
        // Every access point and station is managed: the run's credentials.
        let generated = |prefix: &str| -> Result<(String, String)> {
            Ok((
                oer_tool_service_lab::credentials::ssid(prefix)?
                    .expose()
                    .to_owned(),
                oer_tool_service_lab::credentials::passphrase()?
                    .expose()
                    .to_owned(),
            ))
        };
        let (station_ssid, station_passphrase) = generated("oer-sta")?;
        let (ap_ssid, ap_passphrase) = generated("oer-ap")?;
        validate_credentials("station", &station_ssid, &station_passphrase)?;
        validate_credentials("access-point", &ap_ssid, &ap_passphrase)?;
        if !raw
            .access_point
            .channel_width
            .admits_primary(raw.access_point.channel)
        {
            return Err("HIL access-point channel geometry is invalid".into());
        }
        if !(1..=15).contains(&raw.access_point.client_limit) {
            return Err("HIL access-point client_limit must be in 1..=15".into());
        }
        let (target_address, target_prefix) = parse_cidr(
            "access_point.target_address",
            &raw.access_point.target_address,
        )?;
        let (client_address, client_prefix) = parse_cidr(
            "access_point.client_address",
            &raw.access_point.client_address,
        )?;
        if target_prefix != client_prefix
            || target_address == client_address
            || subnet(target_address, target_prefix) != subnet(client_address, client_prefix)
        {
            return Err(
                "HIL AP target/client addresses must be distinct hosts in one IPv4 subnet".into(),
            );
        }
        let secondary_client_address = raw
            .access_point
            .secondary_client_address
            .as_deref()
            .map(|address| parse_cidr("access_point.secondary_client_address", address))
            .transpose()?;
        if let Some((secondary, prefix)) = secondary_client_address
            && (prefix != target_prefix
                || secondary == target_address
                || secondary == client_address
                || subnet(secondary, prefix) != subnet(target_address, target_prefix))
        {
            return Err(
                "HIL secondary AP client must be a distinct host in the target AP subnet".into(),
            );
        }
        let ipv4 = parse_ipv4(raw.station.ipv4.clone())?;
        match &raw.station_fixture {
            RawStationFixtureConfig::LocalLinux {
                interface,
                phys,
                country,
                channel,
                address,
                ..
            } => {
                validate_shell_token("station_fixture.interface", interface)?;
                if interface != "wlan0" {
                    return Err("the installed local HIL helper currently owns only `wlan0`".into());
                }
                validate_phys("local-linux", phys)?;
                if country.len() != 2 || !country.bytes().all(|byte| byte.is_ascii_uppercase()) {
                    return Err(
                        "local AP country must be a two-letter uppercase country code".into(),
                    );
                }
                if !(1..=13).contains(channel) {
                    return Err("local AP channel must be in 1..=13".into());
                }
                let (address, prefix) = parse_cidr("station_fixture.address", address)?;
                if !(1..=30).contains(&prefix) || address.is_unspecified() || address.is_multicast()
                {
                    return Err(
                        "local AP address requires a unicast subnet with host addresses".into(),
                    );
                }
            }
            RawStationFixtureConfig::OpenWrt {
                radio,
                ap_section,
                channel,
                ssh_target,
                wireless_interface,
                ingress_interface,
                monitor_interface,
                phys,
                independent_laptop_monitor: _,
                read_only: _,
            } => {
                if !(1..=13).contains(channel) {
                    return Err("OpenWrt channel must be in 1..=13".into());
                }
                for (name, value) in [
                    ("station_fixture.radio", radio.as_str()),
                    ("station_fixture.ap_section", ap_section.as_str()),
                    ("station_fixture.ssh_target", ssh_target.as_str()),
                    (
                        "station_fixture.wireless_interface",
                        wireless_interface.as_str(),
                    ),
                    (
                        "station_fixture.ingress_interface",
                        ingress_interface.as_str(),
                    ),
                ] {
                    validate_shell_token(name, value)?;
                }
                if let Some(interface) = monitor_interface {
                    validate_shell_token("station_fixture.monitor_interface", interface)?;
                }
                validate_phys("open-wrt", phys)?;
            }
            RawStationFixtureConfig::External { phys } => validate_phys("external", phys)?,
        }
        let station = StationConfig {
            ssid: Zeroizing::new(station_ssid),
            passphrase: Zeroizing::new(station_passphrase),
            ipv4,
        };
        let access_point = AccessPointConfig {
            ssid: Zeroizing::new(ap_ssid),
            passphrase: Zeroizing::new(ap_passphrase),
            channel: raw.access_point.channel,
            channel_width: raw.access_point.channel_width,
            client_limit: raw.access_point.client_limit,
            target_address,
            client_address,
            secondary_client_address: secondary_client_address.map(|(address, _)| address),
            prefix_length: target_prefix,
        };
        if let Some(legacy) = &raw.legacy_bss
            && (legacy.country.len() != 2
                || !legacy.country.bytes().all(|byte| byte.is_ascii_uppercase()))
        {
            return Err("legacy BSS country must be a two-letter uppercase country code".into());
        }
        Ok(Self {
            path: path.to_owned(),
            air_observer: raw.air_observer,
            legacy_bss: raw.legacy_bss,
            cell_id: stand.identity()?.id.clone(),
            bluetooth_adapter: raw
                .bluetooth
                .map(|config| config.adapter.parse())
                .transpose()?,
            chip: chip.to_owned(),
            stand,
            choice: choice.clone(),
            peer_chip: None,
            dut: DeviceConfig {
                id: device_id,
                mac: device_mac,
                serial: device_serial,
            },
            station,
            access_point,
            station_fixture: match raw.station_fixture {
                RawStationFixtureConfig::LocalLinux {
                    interface,
                    phys,
                    country,
                    channel,
                    address,
                    coexistence,
                } => {
                    let (address, prefix_length) = parse_cidr("station_fixture.address", &address)?;
                    StationFixtureConfig::LocalLinux(LocalLinuxConfig {
                        interface,
                        phys,
                        country,
                        channel,
                        ht40_above: channel <= 9,
                        address,
                        prefix_length,
                        coexistence,
                    })
                }
                RawStationFixtureConfig::OpenWrt {
                    radio,
                    ap_section,
                    channel,
                    ssh_target,
                    wireless_interface,
                    ingress_interface,
                    monitor_interface,
                    phys,
                    independent_laptop_monitor,
                    read_only,
                } => StationFixtureConfig::OpenWrt(OpenWrtConfig {
                    ht40_above: channel <= 9,
                    radio,
                    ap_section,
                    channel,
                    ssh_target,
                    wireless_interface,
                    ingress_interface,
                    monitor_interface,
                    phys,
                    independent_laptop_monitor,
                    read_only,
                }),
                RawStationFixtureConfig::External { phys } => {
                    StationFixtureConfig::External(ExternalConfig { phys })
                }
            },
        })
    }

    /// The station-fixture link a scenario requires: its own link or, for a
    /// scenario without one, the access point's channel width.
    pub fn fixture_phy(&self, wifi: WifiLabUse) -> PhyExpectation {
        wifi.link.unwrap_or_else(|| {
            if self.access_point.bandwidth_mhz() == 40 {
                PhyExpectation::Ht40
            } else {
                PhyExpectation::Ht20
            }
        })
    }

    /// The frequency range, in kHz, the Wi-Fi link of `wifi` occupies: the
    /// fixture's primary channel and, for HT40, its secondary channel above
    /// or below, with the spectral mask's 1 MHz beyond each 20 MHz channel.
    pub fn wifi_range_khz(&self, wifi: WifiLabUse) -> (u64, u64) {
        let lab = self.resolve(wifi);
        let (channel, above) = match &lab.station_fixture {
            StationFixtureConfig::OpenWrt(config) => (config.channel, config.ht40_above),
            StationFixtureConfig::LocalLinux(config) => (config.channel, config.ht40_above),
            StationFixtureConfig::External(_) => (
                lab.access_point.channel,
                lab.access_point.channel_width == WifiChannelWidth::Mhz40Above,
            ),
        };
        let center: u64 = if channel == 14 {
            2_484_000
        } else {
            2_407_000 + 5_000 * u64::from(channel)
        };
        match lab.fixture_phy(wifi) {
            PhyExpectation::Ht40 if above => (center - 11_000, center + 31_000),
            PhyExpectation::Ht40 => (center - 31_000, center + 11_000),
            PhyExpectation::Legacy | PhyExpectation::Ht20 | PhyExpectation::He20 => {
                (center - 11_000, center + 11_000)
            }
        }
    }

    /// Resolve the channel geometry of an access-point scenario: the target
    /// AP adopts the requested link and the fixture follows its channel.
    pub fn resolve(&self, wifi: WifiLabUse) -> Self {
        let mut lab = self.clone();
        if wifi.access_point {
            if let Some(link) = wifi.link {
                lab.access_point.channel_width = match link {
                    PhyExpectation::Legacy | PhyExpectation::Ht20 | PhyExpectation::He20 => {
                        WifiChannelWidth::Mhz20
                    }
                    PhyExpectation::Ht40
                        if self.access_point.channel_width.bandwidth_mhz() == 40 =>
                    {
                        self.access_point.channel_width
                    }
                    PhyExpectation::Ht40 if self.access_point.channel <= 9 => {
                        WifiChannelWidth::Mhz40Above
                    }
                    PhyExpectation::Ht40 => WifiChannelWidth::Mhz40Below,
                };
            }
            if let StationFixtureConfig::OpenWrt(config) = &mut lab.station_fixture {
                config.channel = lab.access_point.channel;
                config.ht40_above = lab.access_point.channel_width == WifiChannelWidth::Mhz40Above;
            }
            if let StationFixtureConfig::LocalLinux(config) = &mut lab.station_fixture {
                config.channel = lab.access_point.channel;
                config.ht40_above = lab.access_point.channel_width == WifiChannelWidth::Mhz40Above;
            }
        }
        lab
    }

    /// The chip of the device under test this configuration uses.
    pub fn chip(&self) -> &str {
        &self.chip
    }

    /// This configuration for a run whose scenarios bring their peers up to
    /// the catalog `images`: its peer is a board of their chip
    /// (`hil/peers/*/firmware.toml`). A run's peers share one chip.
    pub fn with_peer_images(mut self, images: &[&str]) -> Result<Self> {
        let root = oer_tool_base_process::built_root();
        let mut chips = images
            .iter()
            .map(|image| peer_image_chip(&root, image))
            .collect::<Result<Vec<_>>>()?;
        chips.sort();
        chips.dedup();
        self.peer_chip = match chips.as_slice() {
            [] => None,
            [chip] => Some(chip.clone()),
            several => {
                return Err(format!(
                    "the run's peer images target {}; run their scenarios separately",
                    several.join(" and ")
                )
                .into());
            }
        };
        Ok(self)
    }

    /// The run's peer board: a board of the run's peer chip that may be a
    /// peer, other than the device under test; the one the run named
    /// (`--peer-board`), or the only one. Its port is resolved when a run
    /// uses it.
    pub fn peer(&self) -> Result<PeerBoardConfig> {
        self.peer_resolving(&attached_port)
    }

    pub(crate) fn peer_resolving(
        &self,
        resolve: &dyn Fn(&oer_tool_data_stand_file::Board) -> Result<PathBuf>,
    ) -> Result<PeerBoardConfig> {
        let chip = self
            .peer_chip
            .as_deref()
            .ok_or("this run's scenarios use no peer")?;
        let board = self
            .stand
            .select_board(
                chip,
                oer_tool_data_stand_file::BoardRole::Peer,
                self.choice.peer.as_deref(),
                Some(&self.dut.id),
            )
            .map_err(|error| format!("{error} (--peer-board)"))?;
        Ok(PeerBoardConfig {
            id: board.id.clone(),
            mac: board.device_id()?.to_string(),
            chip: board.chip.clone(),
            port: resolve(board).map_err(|error| error.to_string()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The stand file the configuration was read from.
    pub fn stand(&self) -> &oer_tool_data_stand_file::Stand {
        &self.stand
    }

    /// The device under test as the stand describes it; its I/O needs the
    /// run's access to it (`Board::lease`).
    pub fn dut_board(&self) -> Result<oer_tool_service_stand_board::Board> {
        let board = self
            .stand
            .board(&self.dut.id)
            .ok_or_else(|| format!("board `{}` is not in the stand file", self.dut.id))?;
        oer_tool_service_stand_board::Board::new(
            &oer_tool_base_process::built_root(),
            &self.stand,
            board,
            self.dut.serial.clone(),
        )
    }

    /// The run's peer board as the stand describes it; its I/O needs the
    /// run's access to it (`Board::lease`).
    pub fn peer_board(&self) -> Result<oer_tool_service_stand_board::Board> {
        let peer = self.peer()?;
        let board = self
            .stand
            .board(&peer.id)
            .ok_or_else(|| format!("board `{}` is not in the stand file", peer.id))?;
        oer_tool_service_stand_board::Board::new(
            &oer_tool_base_process::built_root(),
            &self.stand,
            board,
            peer.serial()?,
        )
    }

    pub fn cell_id(&self) -> &str {
        &self.cell_id
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test() -> Self {
        Self {
            path: PathBuf::from("stand.toml"),
            air_observer: None,
            legacy_bss: None,
            cell_id: String::from("test-cell"),
            bluetooth_adapter: None,
            // A chip of the tree this build came from.
            chip: oer_tool_data_chip_profile::Profile::all(&oer_tool_base_process::built_root())
                .expect("the chip profiles load")
                .into_iter()
                .next()
                .expect("the tree has a chip")
                .id,
            stand: oer_tool_data_stand_file::Stand::parse(
                "schema = 1\n[stand]\nid = \"test-cell\"\nair = \"exclusive\"\n",
            )
            .expect("the test stand file parses"),
            choice: BoardChoice::default(),
            peer_chip: None,
            dut: DeviceConfig {
                id: String::from("test-device"),
                mac: String::from("02:00:00:00:00:FF"),
                serial: PathBuf::from("/dev/ttyACM0"),
            },
            station: StationConfig {
                ssid: Zeroizing::new(String::from("test-network")),
                passphrase: Zeroizing::new(String::from("test-password")),
                ipv4: NetworkIpv4Configuration::Dhcp,
            },
            access_point: AccessPointConfig {
                ssid: Zeroizing::new(String::from("test-device-ap")),
                passphrase: Zeroizing::new(String::from("test-password")),
                channel: 6,
                channel_width: WifiChannelWidth::Mhz40Above,
                client_limit: 4,
                target_address: Ipv4Addr::new(10, 43, 0, 1),
                client_address: Ipv4Addr::new(10, 43, 0, 2),
                secondary_client_address: Some(Ipv4Addr::new(10, 43, 0, 3)),
                prefix_length: 24,
            },
            station_fixture: StationFixtureConfig::OpenWrt(OpenWrtConfig {
                ht40_above: true,
                radio: String::from("radio0"),
                ap_section: String::from("default_radio0"),
                channel: 6,
                ssh_target: String::from("open-radio-ap"),
                wireless_interface: String::from("phy0-ap0"),
                ingress_interface: String::from("br-lan"),
                monitor_interface: Some(String::from("open-radio-mon")),
                phys: vec![PhyExpectation::Ht40],
                independent_laptop_monitor: true,
                read_only: false,
            }),
        }
    }
}

impl StationFixtureConfig {
    pub fn require_phy(&self, phy: PhyExpectation) -> Result<()> {
        let phys = match self {
            Self::LocalLinux(config) => &config.phys,
            Self::OpenWrt(config) => &config.phys,
            Self::External(config) => &config.phys,
        };
        if !phys.contains(&phy) {
            return Err(format!(
                "station fixture does not advertise the required `{}` PHY; configured capabilities: {:?}",
                phy.id(), phys
            )
            .into());
        }
        Ok(())
    }
}

fn validate_phys(owner: &str, phys: &[PhyExpectation]) -> Result<()> {
    if phys.is_empty() {
        return Err(format!("{owner} station fixture must advertise at least one PHY").into());
    }
    for (index, phy) in phys.iter().enumerate() {
        if phys[..index].contains(phy) {
            return Err(format!("{owner} station fixture advertises `{}` twice", phy.id()).into());
        }
    }
    Ok(())
}

impl StationConfig {
    pub const fn ipv4(&self) -> NetworkIpv4Configuration {
        self.ipv4
    }

    pub fn credentials(&self) -> (&str, &str) {
        (self.ssid.as_str(), self.passphrase.as_str())
    }
}

impl AccessPointConfig {
    pub fn credentials(&self) -> (&str, &str) {
        (self.ssid.as_str(), self.passphrase.as_str())
    }

    pub const fn channel(&self) -> u8 {
        self.channel
    }

    pub const fn bandwidth_mhz(&self) -> u16 {
        self.channel_width.bandwidth_mhz()
    }

    pub const fn channel_width(&self) -> WifiChannelWidth {
        self.channel_width
    }

    /// Exact primary-channel frequency used to constrain client scanning.
    pub const fn frequency_mhz(&self) -> u16 {
        2_407 + self.channel as u16 * 5
    }

    pub const fn client_limit(&self) -> u8 {
        self.client_limit
    }

    pub const fn target_address(&self) -> Ipv4Addr {
        self.target_address
    }

    pub const fn client_address(&self) -> Ipv4Addr {
        self.client_address
    }

    pub const fn secondary_client_address(&self) -> Option<Ipv4Addr> {
        self.secondary_client_address
    }

    pub fn client_cidr(&self) -> String {
        format!("{}/{}", self.client_address, self.prefix_length)
    }

    pub fn secondary_client_cidr(&self) -> Option<String> {
        self.secondary_client_address
            .map(|address| format!("{address}/{}", self.prefix_length))
    }
}

fn validate_credentials(name: &str, ssid: &str, passphrase: &str) -> Result<()> {
    if ssid.is_empty() || ssid.len() > 32 || ssid.chars().any(char::is_control) {
        return Err(format!("HIL {name} SSID must contain 1..=32 non-control bytes").into());
    }
    if !(8..=63).contains(&passphrase.len()) || passphrase.chars().any(char::is_control) {
        return Err(format!("HIL {name} passphrase must contain 8..=63 non-control bytes").into());
    }
    Ok(())
}

fn parse_cidr(name: &str, value: &str) -> Result<(Ipv4Addr, u8)> {
    let (address, prefix) = value
        .split_once('/')
        .ok_or_else(|| format!("{name} must be an IPv4 CIDR"))?;
    let address = address
        .parse::<Ipv4Addr>()
        .map_err(|error| format!("invalid {name} address: {error}"))?;
    let prefix = prefix
        .parse::<u8>()
        .map_err(|error| format!("invalid {name} prefix: {error}"))?;
    if prefix > 30 || address.is_unspecified() || address.is_broadcast() {
        return Err(format!("{name} must identify a host in a /0..=/30 subnet").into());
    }
    let host_mask = if prefix == 0 {
        u32::MAX
    } else {
        u32::MAX >> prefix
    };
    let host = u32::from(address) & host_mask;
    if host == 0 || host == host_mask {
        return Err(format!("{name} cannot be the subnet or broadcast address").into());
    }
    Ok((address, prefix))
}

fn subnet(address: Ipv4Addr, prefix: u8) -> u32 {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    u32::from(address) & mask
}

fn parse_ipv4(raw: RawIpv4Config) -> Result<NetworkIpv4Configuration> {
    let configuration = match raw {
        RawIpv4Config::Dhcp => NetworkIpv4Configuration::Dhcp,
        RawIpv4Config::Static { address, gateway } => {
            let (address, prefix) = address
                .split_once('/')
                .ok_or("station.ipv4.address must be an IPv4 CIDR")?;
            NetworkIpv4Configuration::Static {
                address: address
                    .parse::<Ipv4Addr>()
                    .map_err(|error| format!("invalid station IPv4 address: {error}"))?
                    .octets(),
                prefix_length: prefix
                    .parse::<u8>()
                    .map_err(|error| format!("invalid station IPv4 prefix: {error}"))?,
                gateway: gateway.map(|address| address.octets()),
            }
        }
    };
    if !configuration.validate() {
        return Err("invalid station IPv4 configuration".into());
    }
    Ok(configuration)
}

fn validate_shell_token(name: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'@'))
    {
        return Err(format!("{name} contains unsupported characters").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
