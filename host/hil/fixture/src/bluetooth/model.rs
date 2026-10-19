//! Shared, versioned boundary between the unprivileged runner and finite helper.

pub mod security_failure;

use oer_hil_protocol::bluetooth::BluetoothPeripheralTermination;
use serde::{Deserialize, Serialize};

pub use oer_lab_fixtures::bluetooth_contract::{CONNECTION_RESET_SCHEMA, HELPER_CAPABILITIES};

/// The target's LE features as the central's Controller reports them:
/// Encryption, Peripheral Feature Exchange, Ping, LE Data Packet Length
/// Extension and CSA #2, which the central also supports.
///
/// Feature expectations are independent of the production implementation;
/// matching the mask alone does not establish encrypted interoperability.
pub const EXPECTED_REMOTE_FEATURES: [u8; 8] = [0x39, 0x40, 0, 0, 0, 0, 0, 0];

/// The target's Link Layer version, company identifier and subversion.
pub const EXPECTED_REMOTE_VERSION: (u8, u16, u16) = (0x0d, 0xffff, 1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Adapter(pub u16);

impl std::str::FromStr for Adapter {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = || "adapter must be hci0 through hci65534 (canonical decimal)".to_owned();
        let number = value.strip_prefix("hci").ok_or_else(invalid)?;
        let index = number.parse::<u16>().map_err(|_| invalid())?;
        if index == u16::MAX || number != index.to_string() {
            return Err(invalid());
        }
        Ok(Self(index))
    }
}

impl std::fmt::Display for Adapter {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(output, "hci{}", self.0)
    }
}

/// What one DTM check exercises on the adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DtmProfile {
    /// Receive, end with the packet count, then transmit and end.
    #[default]
    ReceiveTransmit,
    /// Transmit and end, for a receiving peer.
    Transmit,
    /// Receive while the peer is silent, then end with HCI Reset: some
    /// adapters never complete LE Test End after a receiver test that heard
    /// nothing.
    ReceiveSilence,
}

impl DtmProfile {
    pub const ALL: [Self; 3] = [Self::ReceiveTransmit, Self::Transmit, Self::ReceiveSilence];

    /// The helper's `--profile` argument.
    pub const fn argument(self) -> &'static str {
        match self {
            Self::ReceiveTransmit => "receive-transmit",
            Self::Transmit => "transmit",
            Self::ReceiveSilence => "receive-silence",
        }
    }
}

/// Explicit command profile; v1 is diagnostic only, never an automatic fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DtmVersion {
    V1,
    V2,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub schema: u32,
    pub adapter: String,
    pub address: Option<String>,
    pub version: Option<String>,
    pub initial_powered: Option<bool>,
    pub initial_soft_blocked: Option<bool>,
    pub dtm_version: DtmVersion,
    pub profile: DtmProfile,
    pub dtm_v1_advertised: bool,
    pub dtm_v2_advertised: bool,
    pub rx_started: bool,
    pub rx_packets: Option<u16>,
    /// The receiver test ended with HCI Reset, so no count exists.
    pub rx_ended_by_reset: bool,
    pub tx_started: bool,
    pub tx_test_end: bool,
    pub restored: bool,
    pub errors: Vec<String>,
}

impl Check {
    pub fn new(adapter: Adapter, dtm_version: DtmVersion, profile: DtmProfile) -> Self {
        Self {
            schema: 3,
            adapter: adapter.to_string(),
            address: None,
            version: None,
            initial_powered: None,
            initial_soft_blocked: None,
            dtm_version,
            profile,
            dtm_v1_advertised: false,
            dtm_v2_advertised: false,
            rx_started: false,
            rx_packets: None,
            rx_ended_by_reset: false,
            tx_started: false,
            tx_test_end: false,
            restored: false,
            errors: Vec::new(),
        }
    }

    /// This proves command acceptance only; packet reception needs an RF peer.
    pub fn passed(&self, adapter: Adapter, dtm_version: DtmVersion, profile: DtmProfile) -> bool {
        let steps = match profile {
            DtmProfile::ReceiveTransmit => {
                self.rx_started
                    && self.rx_packets.is_some()
                    && !self.rx_ended_by_reset
                    && self.tx_started
                    && self.tx_test_end
            }
            DtmProfile::Transmit => !self.rx_started && self.tx_started && self.tx_test_end,
            DtmProfile::ReceiveSilence => {
                self.rx_started
                    && self.rx_packets.is_none()
                    && self.rx_ended_by_reset
                    && !self.tx_started
            }
        };
        self.schema == 3
            && self.dtm_version == dtm_version
            && self.profile == profile
            && self.adapter == adapter.to_string()
            && self.address.is_some()
            && self.version.is_some()
            && self.initial_powered.is_some()
            && self.initial_soft_blocked.is_some()
            && match dtm_version {
                DtmVersion::V1 => self.dtm_v1_advertised,
                DtmVersion::V2 => self.dtm_v2_advertised,
            }
            && steps
            && self.restored
            && self.errors.is_empty()
    }
}

/// One public Bluetooth peer, supplied as six hexadecimal octets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerAddress(pub [u8; 6]);

impl std::str::FromStr for PeerAddress {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = || "peer must be a public address such as 30:ED:A0:F3:F6:D1".to_owned();
        let octets: Vec<_> = value.split(':').collect();
        if octets.len() != 6 {
            return Err(invalid());
        }
        let mut wire = [0; 6];
        for (index, octet) in octets.into_iter().enumerate() {
            if octet.len() != 2 || !octet.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(invalid());
            }
            wire[5 - index] = u8::from_str_radix(octet, 16).map_err(|_| invalid())?;
        }
        Ok(Self(wire))
    }
}

impl std::fmt::Display for PeerAddress {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, byte) in self.0.iter().rev().enumerate() {
            if index != 0 {
                output.write_str(":")?;
            }
            write!(output, "{byte:02X}")?;
        }
        Ok(())
    }
}

/// Central-side connection, ACL echo and selected termination evidence.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionReset {
    pub schema: u32,
    pub adapter: String,
    pub peer: String,
    pub hold_ms: u16,
    pub encrypted: bool,
    pub key_refresh: bool,
    pub refresh_command_status: bool,
    pub key_refresh_complete: bool,
    pub refresh_after_micros: Option<u64>,
    pub encryption_command_status: bool,
    pub encryption_change: bool,
    pub encryption_after_micros: Option<u64>,
    pub termination: BluetoothPeripheralTermination,
    pub initial_powered: Option<bool>,
    pub initial_soft_blocked: Option<bool>,
    pub connection_complete: bool,
    #[serde(default)]
    pub remote_features_command_status: bool,
    #[serde(default)]
    pub remote_features_complete: bool,
    #[serde(default)]
    pub remote_features: Option<[u8; 8]>,
    #[serde(default)]
    pub remote_features_after_micros: Option<u64>,
    #[serde(default)]
    pub remote_version_command_status: bool,
    #[serde(default)]
    pub remote_version_complete: bool,
    #[serde(default)]
    pub remote_version: Option<u8>,
    #[serde(default)]
    pub remote_version_company: Option<u16>,
    #[serde(default)]
    pub remote_version_subversion: Option<u16>,
    #[serde(default)]
    pub remote_version_after_micros: Option<u64>,
    pub acl_sent: bool,
    pub acl_echo_received: bool,
    pub acl_payload_bytes: Option<u16>,
    pub acl_echo_hci_packets: Option<u16>,
    pub acl_echo_after_micros: Option<u64>,
    pub connection_update_complete: bool,
    pub updated_interval_millis: Option<u16>,
    pub connection_update_after_micros: Option<u64>,
    #[serde(default)]
    pub channel_map_updated: bool,
    #[serde(default)]
    pub channel_map_update_after_micros: Option<u64>,
    #[serde(default)]
    pub post_update_acl_sent: bool,
    #[serde(default)]
    pub post_update_acl_echo_received: bool,
    #[serde(default)]
    pub post_update_acl_echo_hci_packets: Option<u16>,
    #[serde(default)]
    pub post_update_acl_echo_after_micros: Option<u64>,
    pub reset_completed: bool,
    pub peer_rfkill_blocked: bool,
    pub peer_rfkill_micros: Option<u64>,
    pub peer_disconnection_complete: bool,
    pub peer_disconnect_reason: Option<u8>,
    pub connection_after_micros: Option<u64>,
    pub termination_after_connection_micros: Option<u64>,
    pub restored: bool,
    pub errors: Vec<String>,
}

impl ConnectionReset {
    pub fn new(
        adapter: Adapter,
        peer: PeerAddress,
        hold_ms: u16,
        termination: BluetoothPeripheralTermination,
    ) -> Self {
        Self {
            schema: CONNECTION_RESET_SCHEMA,
            adapter: adapter.to_string(),
            peer: peer.to_string(),
            hold_ms,
            encrypted: false,
            key_refresh: false,
            refresh_command_status: false,
            key_refresh_complete: false,
            refresh_after_micros: None,
            encryption_command_status: false,
            encryption_change: false,
            encryption_after_micros: None,
            termination,
            initial_powered: None,
            initial_soft_blocked: None,
            connection_complete: false,
            remote_features_command_status: false,
            remote_features_complete: false,
            remote_features: None,
            remote_features_after_micros: None,
            remote_version_command_status: false,
            remote_version_complete: false,
            remote_version: None,
            remote_version_company: None,
            remote_version_subversion: None,
            remote_version_after_micros: None,
            acl_sent: false,
            acl_echo_received: false,
            acl_payload_bytes: None,
            acl_echo_hci_packets: None,
            acl_echo_after_micros: None,
            connection_update_complete: false,
            updated_interval_millis: None,
            connection_update_after_micros: None,
            channel_map_updated: false,
            channel_map_update_after_micros: None,
            post_update_acl_sent: false,
            post_update_acl_echo_received: false,
            post_update_acl_echo_hci_packets: None,
            post_update_acl_echo_after_micros: None,
            reset_completed: false,
            peer_rfkill_blocked: false,
            peer_rfkill_micros: None,
            peer_disconnection_complete: false,
            peer_disconnect_reason: None,
            connection_after_micros: None,
            termination_after_connection_micros: None,
            restored: false,
            errors: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn passed(
        &self,
        adapter: Adapter,
        peer: PeerAddress,
        hold_ms: u16,
        termination: BluetoothPeripheralTermination,
    ) -> bool {
        self.passed_profile(adapter, peer, hold_ms, termination, false, false)
    }

    pub fn passed_profile(
        &self,
        adapter: Adapter,
        peer: PeerAddress,
        hold_ms: u16,
        termination: BluetoothPeripheralTermination,
        encrypted: bool,
        key_refresh: bool,
    ) -> bool {
        let termination_complete = match termination {
            BluetoothPeripheralTermination::PeerReset => {
                self.reset_completed
                    && !self.peer_rfkill_blocked
                    && self.peer_rfkill_micros.is_none()
                    && !self.peer_disconnection_complete
                    && self.peer_disconnect_reason.is_none()
            }
            BluetoothPeripheralTermination::PeerRfkill => {
                !self.reset_completed
                    && self.peer_rfkill_blocked
                    && self
                        .peer_rfkill_micros
                        .is_some_and(|micros| micros >= 2_500_000)
                    && !self.peer_disconnection_complete
                    && self.peer_disconnect_reason.is_none()
            }
            BluetoothPeripheralTermination::TargetDisconnect => {
                !self.reset_completed
                    && !self.peer_rfkill_blocked
                    && self.peer_rfkill_micros.is_none()
                    && self.peer_disconnection_complete
                    && self.peer_disconnect_reason == Some(0x13)
            }
            BluetoothPeripheralTermination::TargetReset => {
                !self.reset_completed
                    && !self.peer_rfkill_blocked
                    && self.peer_rfkill_micros.is_none()
                    && self.peer_disconnection_complete
                    && self.peer_disconnect_reason == Some(0x08)
            }
        };
        (!key_refresh || encrypted)
            && self.key_refresh == key_refresh
            && self.refresh_command_status == key_refresh
            && self.key_refresh_complete == key_refresh
            && self.refresh_after_micros.is_some() == key_refresh
            && self.encrypted == encrypted
            && self.encryption_command_status == encrypted
            && self.encryption_change == encrypted
            && self.encryption_after_micros.is_some() == encrypted
            && self.schema == CONNECTION_RESET_SCHEMA
            && self.adapter == adapter.to_string()
            && self.peer == peer.to_string()
            && self.hold_ms == hold_ms
            && self.termination == termination
            && hold_ms <= 5_000
            && self.initial_powered.is_some()
            && self.initial_soft_blocked.is_some()
            && self.connection_complete
            && self.remote_features_command_status
            && self.remote_features_complete
            && self.remote_features == Some(EXPECTED_REMOTE_FEATURES)
            && self.remote_features_after_micros.is_some()
            && self.remote_version_command_status
            && self.remote_version_complete
            && self.remote_version == Some(EXPECTED_REMOTE_VERSION.0)
            && self.remote_version_company == Some(EXPECTED_REMOTE_VERSION.1)
            && self.remote_version_subversion == Some(EXPECTED_REMOTE_VERSION.2)
            && self.remote_version_after_micros.is_some()
            && self.acl_sent
            && self.acl_echo_received
            && self.acl_payload_bytes
                == Some(oer_hil_protocol::bluetooth::BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES as u16)
            && self.acl_echo_hci_packets
                == Some(if encrypted {
                    11
                } else {
                    oer_hil_protocol::bluetooth::BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS as u16
                })
            && self.acl_echo_after_micros.is_some()
            && self.connection_update_complete
            && self.updated_interval_millis
                == Some(oer_hil_protocol::bluetooth::BLUETOOTH_PERIPHERAL_UPDATED_INTERVAL_MILLIS)
            && self.connection_update_after_micros.is_some()
            && self.channel_map_updated
            && self.channel_map_update_after_micros.is_some()
            && self.post_update_acl_sent
            && self.post_update_acl_echo_received
            && self.post_update_acl_echo_hci_packets
                == Some(if encrypted {
                    11
                } else {
                    oer_hil_protocol::bluetooth::BLUETOOTH_PERIPHERAL_ACL_LL_FRAGMENTS as u16
                })
            && self.post_update_acl_echo_after_micros.is_some()
            && termination_complete
            && self.connection_after_micros.is_some()
            && self.termination_after_connection_micros.is_some()
            && self.restored
            && self.errors.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_argument_is_the_helper_value() {
        use clap::ValueEnum;
        for profile in DtmProfile::ALL {
            assert_eq!(
                DtmProfile::from_str(profile.argument(), false),
                Ok(profile),
                "{profile:?}"
            );
        }
        assert_eq!(DtmProfile::ALL.len(), DtmProfile::value_variants().len());
    }
    #[test]
    fn adapter_rejects_paths_options_and_reserved_index() {
        for value in [
            "../hci0", "--help", "hci-1", "hci65535", "hci01", "hci+1", "hci",
        ] {
            assert!(value.parse::<Adapter>().is_err(), "{value}");
        }
        assert_eq!("hci0".parse::<Adapter>().unwrap(), Adapter(0));
        assert_eq!("hci65534".parse::<Adapter>().unwrap(), Adapter(65534));
    }
    #[test]
    fn mask_alone_and_incomplete_cleanup_cannot_pass() {
        let mut report = Check::new(Adapter(0), DtmVersion::V2, DtmProfile::ReceiveTransmit);
        report.dtm_v2_advertised = true;
        assert!(!report.passed(Adapter(0), DtmVersion::V2, DtmProfile::ReceiveTransmit));
        report.address = Some("test".into());
        report.version = Some("test".into());
        report.initial_powered = Some(false);
        report.initial_soft_blocked = Some(true);
        report.rx_started = true;
        report.rx_packets = Some(0);
        report.tx_started = true;
        report.tx_test_end = true;
        assert!(!report.passed(Adapter(0), DtmVersion::V2, DtmProfile::ReceiveTransmit));
        report.restored = true;
        assert!(report.passed(Adapter(0), DtmVersion::V2, DtmProfile::ReceiveTransmit));
        assert!(!report.passed(Adapter(1), DtmVersion::V2, DtmProfile::ReceiveTransmit));
        report.schema = 1;
        assert!(!report.passed(Adapter(0), DtmVersion::V2, DtmProfile::ReceiveTransmit));
        report.schema = 3;
        report.dtm_v1_advertised = true;
        assert!(!report.passed(Adapter(0), DtmVersion::V1, DtmProfile::ReceiveTransmit));
        report.dtm_version = DtmVersion::V1;
        assert!(report.passed(Adapter(0), DtmVersion::V1, DtmProfile::ReceiveTransmit));
        assert!(!report.passed(Adapter(0), DtmVersion::V2, DtmProfile::ReceiveTransmit));
        report.dtm_v1_advertised = false;
        assert!(!report.passed(Adapter(0), DtmVersion::V1, DtmProfile::ReceiveTransmit));
        report.dtm_version = DtmVersion::V2;
        report.errors.push("reset failed".into());
        assert!(!report.passed(Adapter(0), DtmVersion::V2, DtmProfile::ReceiveTransmit));
    }

    #[test]
    fn each_profile_requires_exactly_its_own_steps() {
        let base = || {
            let mut report = Check::new(Adapter(0), DtmVersion::V2, DtmProfile::Transmit);
            report.address = Some("test".into());
            report.version = Some("test".into());
            report.initial_powered = Some(false);
            report.initial_soft_blocked = Some(true);
            report.dtm_v2_advertised = true;
            report.restored = true;
            report
        };
        let mut transmit = base();
        transmit.tx_started = true;
        transmit.tx_test_end = true;
        assert!(transmit.passed(Adapter(0), DtmVersion::V2, DtmProfile::Transmit));
        assert!(!transmit.passed(Adapter(0), DtmVersion::V2, DtmProfile::ReceiveTransmit));

        let mut silence = base();
        silence.profile = DtmProfile::ReceiveSilence;
        silence.rx_started = true;
        // A silent receiver ended by Reset has no count, and says so.
        assert!(!silence.passed(Adapter(0), DtmVersion::V2, DtmProfile::ReceiveSilence));
        silence.rx_ended_by_reset = true;
        assert!(silence.passed(Adapter(0), DtmVersion::V2, DtmProfile::ReceiveSilence));
        silence.rx_packets = Some(0);
        assert!(!silence.passed(Adapter(0), DtmVersion::V2, DtmProfile::ReceiveSilence));
    }
}

#[cfg(test)]
mod connection_reset_tests {
    use super::*;

    fn complete_remote_information(report: &mut ConnectionReset) {
        report.remote_features_command_status = true;
        report.remote_features_complete = true;
        report.remote_features = Some([0x39, 0x40, 0, 0, 0, 0, 0, 0]);
        report.remote_features_after_micros = Some(1);
        report.remote_version_command_status = true;
        report.remote_version_complete = true;
        report.remote_version = Some(0x0d);
        report.remote_version_company = Some(0xffff);
        report.remote_version_subversion = Some(1);
        report.remote_version_after_micros = Some(1);
    }

    #[test]
    fn reset_report_requires_the_exact_request_and_complete_restoration() {
        let adapter = Adapter(0);
        let peer: PeerAddress = "30:ed:a0:f3:f6:d1".parse().unwrap();
        assert_eq!(peer.to_string(), "30:ED:A0:F3:F6:D1");
        let mut report =
            ConnectionReset::new(adapter, peer, 0, BluetoothPeripheralTermination::PeerReset);
        assert!(!report.passed(adapter, peer, 0, BluetoothPeripheralTermination::PeerReset));
        report.initial_powered = Some(false);
        report.initial_soft_blocked = Some(true);
        report.connection_complete = true;
        complete_remote_information(&mut report);
        report.acl_sent = true;
        report.acl_echo_received = true;
        report.acl_payload_bytes =
            Some(oer_hil_protocol::bluetooth::BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES as u16);
        report.acl_echo_hci_packets = Some(10);
        report.acl_echo_after_micros = Some(200);
        report.connection_update_complete = true;
        report.updated_interval_millis =
            Some(oer_hil_protocol::bluetooth::BLUETOOTH_PERIPHERAL_UPDATED_INTERVAL_MILLIS);
        report.connection_update_after_micros = Some(300);
        report.channel_map_updated = true;
        report.channel_map_update_after_micros = Some(400);
        report.post_update_acl_sent = true;
        report.post_update_acl_echo_received = true;
        report.post_update_acl_echo_hci_packets = Some(10);
        report.post_update_acl_echo_after_micros = Some(500);
        report.connection_after_micros = Some(100);
        report.termination_after_connection_micros = Some(1);
        report.reset_completed = true;
        assert!(!report.passed(adapter, peer, 0, BluetoothPeripheralTermination::PeerReset));
        report.restored = true;
        assert!(report.passed(adapter, peer, 0, BluetoothPeripheralTermination::PeerReset));
        assert!(!report.passed_profile(
            adapter,
            peer,
            0,
            BluetoothPeripheralTermination::PeerReset,
            true,
            false
        ));
        report.encrypted = true;
        report.acl_echo_hci_packets = Some(11);
        report.post_update_acl_echo_hci_packets = Some(11);
        report.encryption_command_status = true;
        report.encryption_change = true;
        assert!(!report.passed_profile(
            adapter,
            peer,
            0,
            BluetoothPeripheralTermination::PeerReset,
            true,
            false
        ));
        report.encryption_after_micros = Some(50);
        assert!(report.passed_profile(
            adapter,
            peer,
            0,
            BluetoothPeripheralTermination::PeerReset,
            true,
            false
        ));
        assert!(!report.passed(adapter, peer, 0, BluetoothPeripheralTermination::PeerReset));
        // Refresh cannot be inferred from initial encryption or an incomplete refresh report.
        assert!(!report.passed_profile(
            adapter,
            peer,
            0,
            BluetoothPeripheralTermination::PeerReset,
            true,
            true
        ));
        report.key_refresh = true;
        report.refresh_command_status = true;
        report.key_refresh_complete = true;
        assert!(!report.passed_profile(
            adapter,
            peer,
            0,
            BluetoothPeripheralTermination::PeerReset,
            true,
            true
        ));
        report.refresh_after_micros = Some(60);
        assert!(report.passed_profile(
            adapter,
            peer,
            0,
            BluetoothPeripheralTermination::PeerReset,
            true,
            true
        ));
        assert!(!report.passed_profile(
            adapter,
            peer,
            0,
            BluetoothPeripheralTermination::PeerReset,
            true,
            false
        ));
        report.post_update_acl_echo_received = false;
        assert!(!report.passed_profile(
            adapter,
            peer,
            0,
            BluetoothPeripheralTermination::PeerReset,
            true,
            true
        ));
        report.post_update_acl_echo_received = true;
        report.key_refresh = false;
        report.refresh_command_status = false;
        report.key_refresh_complete = false;
        report.refresh_after_micros = None;
        report.encrypted = false;
        report.acl_echo_hci_packets = Some(10);
        report.post_update_acl_echo_hci_packets = Some(10);
        report.encryption_command_status = false;
        report.encryption_change = false;
        report.encryption_after_micros = None;
        // An old unencrypted profile and an old helper report are not accepted.
        report.remote_features = Some([0x18, 0x40, 0, 0, 0, 0, 0, 0]);
        assert!(!report.passed(adapter, peer, 0, BluetoothPeripheralTermination::PeerReset));
        report.remote_features = Some([0x39, 0x40, 0, 0, 0, 0, 0, 0]);
        report.schema = 9;
        assert!(!report.passed(adapter, peer, 0, BluetoothPeripheralTermination::PeerReset));
        report.schema = CONNECTION_RESET_SCHEMA;
        report.remote_features = Some([0x18, 0, 0, 0, 0, 0, 0, 0]);
        assert!(!report.passed(adapter, peer, 0, BluetoothPeripheralTermination::PeerReset));
        report.remote_features = Some([0x39, 0x40, 0, 0, 0, 0, 0, 0]);
        report.remote_version_subversion = Some(2);
        assert!(!report.passed(adapter, peer, 0, BluetoothPeripheralTermination::PeerReset));
        report.remote_version_subversion = Some(1);
        report.acl_echo_hci_packets = Some(1);
        assert!(!report.passed(adapter, peer, 0, BluetoothPeripheralTermination::PeerReset));
        report.acl_echo_hci_packets = Some(10);
        report.schema = 4;
        assert!(!report.passed(adapter, peer, 0, BluetoothPeripheralTermination::PeerReset));
        report.schema = 9;
        assert!(!report.passed(adapter, peer, 1, BluetoothPeripheralTermination::PeerReset));
        assert!(!report.passed(
            Adapter(1),
            peer,
            0,
            BluetoothPeripheralTermination::PeerReset
        ));
        assert!(!report.passed(
            adapter,
            "30:ED:A0:F3:F6:D2".parse().unwrap(),
            0,
            BluetoothPeripheralTermination::PeerReset
        ));
        report.errors.push("Reset failed".into());
        assert!(!report.passed(adapter, peer, 0, BluetoothPeripheralTermination::PeerReset));
    }

    #[test]
    fn target_termination_requires_the_exact_peer_reason() {
        let adapter = Adapter(0);
        let peer: PeerAddress = "30:ed:a0:f3:f6:d1".parse().unwrap();
        for (termination, reason) in [
            (BluetoothPeripheralTermination::TargetDisconnect, 0x13),
            (BluetoothPeripheralTermination::TargetReset, 0x08),
        ] {
            let mut report = ConnectionReset::new(adapter, peer, 0, termination);
            report.initial_powered = Some(false);
            report.initial_soft_blocked = Some(false);
            report.connection_complete = true;
            complete_remote_information(&mut report);
            report.acl_sent = true;
            report.acl_echo_received = true;
            report.acl_payload_bytes =
                Some(oer_hil_protocol::bluetooth::BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES as u16);
            report.acl_echo_hci_packets = Some(10);
            report.acl_echo_after_micros = Some(1);
            report.connection_update_complete = true;
            report.updated_interval_millis =
                Some(oer_hil_protocol::bluetooth::BLUETOOTH_PERIPHERAL_UPDATED_INTERVAL_MILLIS);
            report.connection_update_after_micros = Some(1);
            report.channel_map_updated = true;
            report.channel_map_update_after_micros = Some(1);
            report.post_update_acl_sent = true;
            report.post_update_acl_echo_received = true;
            report.post_update_acl_echo_hci_packets = Some(10);
            report.post_update_acl_echo_after_micros = Some(1);
            report.connection_after_micros = Some(1);
            report.termination_after_connection_micros = Some(1);
            report.peer_disconnection_complete = true;
            report.peer_disconnect_reason = Some(reason);
            report.restored = true;
            assert!(report.passed(adapter, peer, 0, termination));
            report.peer_disconnect_reason = Some(reason ^ 1);
            assert!(!report.passed(adapter, peer, 0, termination));
        }
    }

    #[test]
    fn peer_rfkill_requires_the_exact_outage_evidence() {
        let adapter = Adapter(0);
        let peer: PeerAddress = "30:ed:a0:f3:f6:d1".parse().unwrap();
        let mut report =
            ConnectionReset::new(adapter, peer, 0, BluetoothPeripheralTermination::PeerRfkill);
        report.initial_powered = Some(false);
        report.initial_soft_blocked = Some(false);
        report.connection_complete = true;
        complete_remote_information(&mut report);
        report.acl_sent = true;
        report.acl_echo_received = true;
        report.acl_payload_bytes =
            Some(oer_hil_protocol::bluetooth::BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES as u16);
        report.acl_echo_hci_packets = Some(10);
        report.acl_echo_after_micros = Some(1);
        report.connection_update_complete = true;
        report.updated_interval_millis =
            Some(oer_hil_protocol::bluetooth::BLUETOOTH_PERIPHERAL_UPDATED_INTERVAL_MILLIS);
        report.connection_update_after_micros = Some(1);
        report.channel_map_updated = true;
        report.channel_map_update_after_micros = Some(1);
        report.post_update_acl_sent = true;
        report.post_update_acl_echo_received = true;
        report.post_update_acl_echo_hci_packets = Some(10);
        report.post_update_acl_echo_after_micros = Some(1);
        report.connection_after_micros = Some(1);
        report.termination_after_connection_micros = Some(1);
        report.peer_rfkill_blocked = true;
        report.peer_rfkill_micros = Some(2_500_000);
        report.restored = true;
        assert!(report.passed(adapter, peer, 0, BluetoothPeripheralTermination::PeerRfkill));
        report.peer_rfkill_micros = Some(2_499_999);
        assert!(!report.passed(adapter, peer, 0, BluetoothPeripheralTermination::PeerRfkill));
    }
}
