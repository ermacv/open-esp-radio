//! Shared, versioned boundary between the unprivileged runner and finite helper.

use serde::{Deserialize, Serialize};

/// What the privileged helper answers to its capability query. It reports what
/// the target answered; the unprivileged runner judges it against the target
/// profile, so a target change never requires a reinstallation.
pub const HELPER_CAPABILITIES: &str = "schema=22 dtm-check=v1,v2 dtm-profile=receive-transmit,transmit,receive-silence att-parameters=7.5ms,restore";

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
