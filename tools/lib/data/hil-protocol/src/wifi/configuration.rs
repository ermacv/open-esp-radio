//! Wi-Fi initialization: credentials, IPv4 policy and data-path policies.

use core::fmt;

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

// Keep the largest protocol enum comfortably below one RX frame. This value
// bounds executor poll-stack pressure as well as wire latency; complete MPDUs
// are reconstructed from ordered chunks on the host.
pub const WIFI_MONITOR_FRAME_CHUNK_MAX_LEN: usize = 160;
pub const WPA2_SSID_MAX_LEN: usize = 32;
pub const WPA2_PASSPHRASE_MIN_LEN: usize = 8;
pub const WPA2_PASSPHRASE_MAX_LEN: usize = 63;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkCredentialsError {
    SsidLength,
    PassphraseLength,
}

impl fmt::Display for NetworkCredentialsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SsidLength => formatter.write_str("SSID must contain 1..=32 bytes"),
            Self::PassphraseLength => {
                formatter.write_str("WPA2 passphrase must contain 8..=63 bytes")
            }
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct NetworkCredentials {
    ssid: [u8; WPA2_SSID_MAX_LEN],
    ssid_length: u8,
    passphrase: heapless::Vec<u8, WPA2_PASSPHRASE_MAX_LEN>,
}

impl NetworkCredentials {
    pub fn try_new(ssid: &[u8], passphrase: &[u8]) -> Result<Self, NetworkCredentialsError> {
        if ssid.is_empty() || ssid.len() > WPA2_SSID_MAX_LEN {
            return Err(NetworkCredentialsError::SsidLength);
        }
        if !(WPA2_PASSPHRASE_MIN_LEN..=WPA2_PASSPHRASE_MAX_LEN).contains(&passphrase.len()) {
            return Err(NetworkCredentialsError::PassphraseLength);
        }
        let mut credentials = Self {
            ssid: [0; WPA2_SSID_MAX_LEN],
            ssid_length: ssid.len() as u8,
            passphrase: heapless::Vec::new(),
        };
        credentials.ssid[..ssid.len()].copy_from_slice(ssid);
        credentials
            .passphrase
            .extend_from_slice(passphrase)
            .map_err(|_| NetworkCredentialsError::PassphraseLength)?;
        Ok(credentials)
    }

    pub fn validate(&self) -> Result<(), NetworkCredentialsError> {
        let ssid_length = usize::from(self.ssid_length);
        if ssid_length == 0 || ssid_length > self.ssid.len() {
            return Err(NetworkCredentialsError::SsidLength);
        }
        if !(WPA2_PASSPHRASE_MIN_LEN..=WPA2_PASSPHRASE_MAX_LEN).contains(&self.passphrase.len()) {
            return Err(NetworkCredentialsError::PassphraseLength);
        }
        Ok(())
    }

    pub fn ssid(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_length)]
    }

    pub fn passphrase(&self) -> &[u8] {
        self.passphrase.as_slice()
    }

    pub fn clear_passphrase(&mut self) {
        self.passphrase.as_mut_slice().zeroize();
        self.passphrase.clear();
    }
}

impl fmt::Debug for NetworkCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkCredentials")
            .field("ssid_length", &self.ssid_length)
            .field("passphrase", &"<redacted>")
            .finish()
    }
}

impl Drop for NetworkCredentials {
    fn drop(&mut self) {
        self.ssid.zeroize();
        self.ssid_length = 0;
        self.clear_passphrase();
    }
}

/// IPv4 policy selected by the host for this boot.
///
/// Keeping this in startup provisioning lets one qualified firmware image run
/// against both an ordinary DHCP network and an isolated HIL access point.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum NetworkIpv4Configuration {
    Dhcp,
    Static {
        address: [u8; 4],
        prefix_length: u8,
        gateway: Option<[u8; 4]>,
    },
}

impl NetworkIpv4Configuration {
    pub fn validate(self) -> bool {
        match self {
            Self::Dhcp => true,
            Self::Static {
                address,
                prefix_length,
                gateway,
            } => {
                prefix_length <= 32
                    && address != [0, 0, 0, 0]
                    && address != [255, 255, 255, 255]
                    && match gateway {
                        Some(gateway) => gateway != [0, 0, 0, 0] && gateway != [255, 255, 255, 255],
                        None => true,
                    }
            }
        }
    }
}

/// The power save a started station runs, as the product's
/// `oer::wifi::StationPowerMode`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "kebab-case")]
pub enum WifiStationPowerSave {
    /// No power save of its own (`WIFI_PS_NONE`).
    #[default]
    None,
    /// Modem sleep waking for every DTIM (`WIFI_PS_MIN_MODEM`).
    MinModem,
    /// Modem sleep waking at the listen interval, in beacon intervals
    /// (`WIFI_PS_MAX_MODEM`); zero is refused.
    MaxModem { listen_interval: u16 },
}

impl WifiStationPowerSave {
    /// Whether the station can run it: a max-modem listen interval of zero
    /// beacon intervals cannot.
    pub const fn is_valid(self) -> bool {
        !matches!(self, Self::MaxModem { listen_interval: 0 })
    }
}

/// A station start: the network and the station's power save.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationStart {
    pub credentials: NetworkCredentials,
    pub power_save: WifiStationPowerSave,
}

impl core::fmt::Debug for StationStart {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("StationStart")
            .field("credentials", &self.credentials)
            .field("power_save", &self.power_save)
            .finish()
    }
}

/// Executor placement selected once, before any Wi-Fi worker or IP stack is
/// materialized. Radio and RX protocol ownership remain on CPU0 in both modes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "kebab-case")]
pub enum WifiDataPlanePlacement {
    /// Radio, RX protocol, IP stack and socket workloads share CPU0.
    SingleCore,
    /// Only the IP stack and socket workloads move to CPU1.
    #[default]
    SplitRadioNetwork,
}

/// IPv4/UDP receive checksum policy selected before the network stack starts.
///
/// The diagnostic variant exists only for a same-image HIL cost experiment;
/// it is not a claim that the Wi-Fi MAC performs transport checksum offload.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "kebab-case")]
pub enum WifiRxChecksumPolicy {
    /// Validate IPv4 and UDP receive checksums in the software IP stack.
    #[default]
    Software,
    /// Trust the isolated HIL traffic generator and skip IPv4/UDP RX checks.
    AssumeValidDiagnostic,
}

/// IPv4 UDP transmit checksum policy selected before the network stack starts.
///
/// IPv4 permits a zero UDP checksum. The diagnostic variant uses that wire
/// representation to isolate software checksum cost without claiming hardware
/// offload or disabling the mandatory IPv4 header checksum.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "kebab-case")]
pub enum WifiTxUdpChecksumPolicy {
    /// Generate the IPv4 UDP checksum in the software IP stack.
    #[default]
    Software,
    /// Emit a zero IPv4 UDP checksum for a same-image HIL cost experiment.
    OmitIpv4Diagnostic,
}

/// TX storage policy selected for a same-image hardware experiment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "kebab-case")]
pub enum WifiTxBufferPolicy {
    /// Queue an owned general-memory packet, select its radio flow, then copy
    /// it once into the fixed internal-SRAM execution pool.
    #[default]
    OwnedSramPromotion,
    /// Keep owned-packet promotion, but have the HIL producer publish one bounded
    /// destination-homogeneous burst at a time. This isolates packet-selection
    /// order from physical SRAM capacity without changing the radio datapath.
    OwnedSramPromotionBurstDiagnostic,
    /// Keep Wi-Fi descriptors in internal SRAM but publish PSRAM packet-buffer
    /// addresses after an explicit cache writeback. Hardware support is not
    /// assumed; this value exists only for the bounded DMA-address HIL.
    PsramDirectDmaDiagnostic,
}

/// Continuation policy for a masked RX drain epoch in one coarse image.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "kebab-case")]
pub enum WifiRxContinuationPolicy {
    /// Preserve the production immediate software repost.
    #[default]
    ImmediateSoftwareProbe,
    /// Restore the level-triggered source after a recycled-only turn.
    LevelIrqDiagnostic,
    /// Retain source masking and repoll after 64 microseconds.
    DelayedProbe64Diagnostic,
    /// Retain source masking and repoll after 128 microseconds.
    DelayedProbe128Diagnostic,
    /// Retain source masking and repoll after 256 microseconds.
    DelayedProbe256Diagnostic,
    /// Retain source masking and repoll after 512 microseconds.
    DelayedProbe512Diagnostic,
    /// Retain source masking and repoll after 1024 microseconds.
    DelayedProbe1024Diagnostic,
    /// Select a bounded window from the completed physical batch geometry.
    AdaptiveProbeDiagnostic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct InitializationConfiguration {
    pub ap_scheduler: WifiApScheduler,
    pub ipv4: NetworkIpv4Configuration,
    pub data_plane: WifiDataPlanePlacement,
    pub rx_checksum: WifiRxChecksumPolicy,
    pub tx_udp_checksum: WifiTxUdpChecksumPolicy,
    pub tx_buffer: WifiTxBufferPolicy,
    pub rx_continuation: WifiRxContinuationPolicy,
    pub l1_cache_counters: bool,
}

/// Standalone AP scheduling experiment with one explicit response envelope.
/// Both arms use 3000-us quantum, 100-us minimum and a 32-byte OFDM24 response
/// plus 10-us SIFS per unicast publication. This is not measured airtime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
#[serde(rename_all = "kebab-case")]
pub enum WifiApScheduler {
    #[default]
    Disabled,
    RrHtResponse24,
    DeficitHtResponse24,
}

impl InitializationConfiguration {
    pub fn validate(self) -> bool {
        self.ipv4.validate()
    }
}

/// The target initialization a host session applies: the role-neutral
/// [`InitializationConfiguration`] without the network addresses, which come
/// from the station network, and the power save a station start selects. A
/// HIL scenario declares it; the host link sends it.
#[cfg(feature = "registry")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetSettings {
    pub ap_scheduler: WifiApScheduler,
    pub data_plane: WifiDataPlanePlacement,
    pub rx_checksum: WifiRxChecksumPolicy,
    pub tx_udp_checksum: WifiTxUdpChecksumPolicy,
    pub tx_buffer: WifiTxBufferPolicy,
    pub rx_continuation: WifiRxContinuationPolicy,
    pub l1_cache_counters: bool,
    /// The power save the station runs once started.
    pub station_power_save: WifiStationPowerSave,
}

#[cfg(feature = "registry")]
impl Default for TargetSettings {
    fn default() -> Self {
        Self {
            ap_scheduler: Default::default(),
            data_plane: WifiDataPlanePlacement::SplitRadioNetwork,
            rx_checksum: WifiRxChecksumPolicy::Software,
            tx_udp_checksum: WifiTxUdpChecksumPolicy::Software,
            tx_buffer: WifiTxBufferPolicy::OwnedSramPromotion,
            rx_continuation: WifiRxContinuationPolicy::ImmediateSoftwareProbe,
            l1_cache_counters: false,
            station_power_save: WifiStationPowerSave::None,
        }
    }
}

#[cfg(feature = "registry")]
impl TargetSettings {
    /// The initialization message of these settings for a station network's
    /// addresses.
    pub fn initialization(self, ipv4: NetworkIpv4Configuration) -> InitializationConfiguration {
        InitializationConfiguration {
            ap_scheduler: self.ap_scheduler,
            ipv4,
            data_plane: self.data_plane,
            rx_checksum: self.rx_checksum,
            tx_udp_checksum: self.tx_udp_checksum,
            tx_buffer: self.tx_buffer,
            rx_continuation: self.rx_continuation,
            l1_cache_counters: self.l1_cache_counters,
        }
    }
}
