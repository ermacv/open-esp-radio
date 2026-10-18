//! Concrete ESP32-S31 transition from a selected scan record to one
//! programmed associated peer.
//!
//! Authentication and Association are finite protocol runners. This module
//! owns the driver transition around them: install scan-time TX policy before
//! the request, complete the peer plan from the successful response, program
//! the HE/rate-control hardware leaves, and return the exact connected-peer
//! state consumed by a runtime integration. Applications and HIL may observe
//! the returned report, but do not reproduce these policy or hardware
//! decisions.

use crate::peer_policy::{StaPeerAssociationPlanError, StaPeerScanPolicy, StaWmmSource};

use oer_esp32s31_hal::types::MacHeBeamformingReportProfileError;

use oer_esp32s31_ieee80211_mac::{
    edca::EdcaParametersError,
    he::{He20InstallError, He20PeerHardware, program_he20_peer_state},
    init::StaNoiseFloorHardware,
    rate::control::{BeamformingReportHardware, StaRateControlTx},
    tx::{HtPeerAmpduParameters, protection::BssProtection},
};
use oer_espressif_ieee80211_policy::rate_control::{StaLinkMetric, StaRateControlAssociation};

use {
    oer_ieee80211_mac::extensions::wmm::WmmParameterSet, oer_ieee80211_mac::he::He20Capabilities,
    oer_ieee80211_mac::he::He20PeerState, oer_ieee80211_mac::he::HeDcmConstellation,
    oer_ieee80211_mac::ht::HtPeerCapabilities, oer_ieee80211_mac::scan::ScanRecord,
    oer_ieee80211_mac::station::AssociationResponse,
    oer_ieee80211_mac::station::association::PhyMode,
};

/// TX-policy capability consumed by the associated-peer transition.
///
/// Keeping this smaller than a complete TX backend makes the ordering testable
/// without a DMA slot and prevents the port from acquiring unrelated runtime
/// responsibilities.
pub trait StaPeerTransmit {
    fn install_ht_ampdu_policy(&mut self, parameters: HtPeerAmpduParameters);

    fn install_he_bss_color(&mut self, bss_color: u8);

    fn install_wmm_edca(&mut self, parameters: WmmParameterSet) -> Result<(), EdcaParametersError>;

    fn install_bss_protection(&mut self, protection: BssProtection);
}

/// Opaque proof that scan-time policy was derived and installed for this
/// candidate before Association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedStaPeer {
    policy: StaPeerScanPolicy,
    access_point: ScanRecord,
}

/// Named mutable capabilities used by final associated-peer programming.
pub struct StaPeerRadio<'a, H, T> {
    pub hardware: &'a mut H,
    pub transmit: &'a mut T,
}

impl<'a, H, T> StaPeerRadio<'a, H, T> {
    pub const fn new(hardware: &'a mut H, transmit: &'a mut T) -> Self {
        Self { hardware, transmit }
    }
}

/// Immutable station/candidate inputs for final peer programming.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaPeerStation {
    pub station_address: [u8; 6],
    pub association_phy: PhyMode,
    /// The association request's RSN element negotiated management frame
    /// protection.
    pub management_protection: bool,
    /// SAE authenticated the association (WPA3-Personal).
    pub sae: bool,
}

impl StaPeerStation {
    pub const fn new(
        station_address: [u8; 6],
        association_phy: PhyMode,
        management_protection: bool,
        sae: bool,
    ) -> Self {
        Self {
            station_address,
            association_phy,
            management_protection,
            sae,
        }
    }
}

/// Stable connected-link facts derived from the selected candidate and its
/// successful Association Response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaConnectedLink {
    pub station_address: [u8; 6],
    pub bssid: [u8; 6],
    /// The SSID the station associated with: the configured SSID the
    /// selected candidate matched (`ScanTable` selects by it).
    pub ssid: oer_ieee80211_mac::ssid::WifiSsid,
    pub association_id: u16,
    pub beacon_interval_tu: u16,
    /// The access point's timestamp in the beacon or probe response the
    /// station joined from; power management places its TBTT from it.
    pub beacon_timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant,
    /// The station's monotonic time when that frame arrived; `None` when
    /// its receive timestamp had no place in the clock relation.
    pub beacon_received_at: Option<oer_time::Instant>,
    pub peer_qos: bool,
    /// The association protects its robust management frames.
    pub management_protection: bool,
    /// SAE authenticated the association (WPA3-Personal).
    pub sae: bool,
    pub association_phy: PhyMode,
    /// The selected HT channel width is allowed to use a 400 ns guard
    /// interval according to the AP's retained HT Capabilities IE.
    pub peer_supports_ht_short_guard_interval: bool,
    /// The selected HT40 peer advertised the independent MCS32 receive bit.
    /// This fact is retained for diagnostics and future policy only: the S31
    /// hardware TX encoding is not yet oracle-qualified.
    pub peer_supports_ht_duplicate_mcs32: bool,
    pub peer_supports_one_ltf_800ns_gi: bool,
    pub peer_supports_ldpc: bool,
    pub peer_dcm_receive: HeDcmConstellation,
}

/// Driver-owned state retained for the connected epoch.
#[derive(Debug, Eq, PartialEq)]
pub struct ConnectedStaPeer {
    pub link: StaConnectedLink,
    pub rate_control: StaRateControlAssociation,
}

/// Value-only observations returned beside the connected owner.
///
/// These fields allow HIL and applications to report the selected policy
/// without inserting callbacks into the programming transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaPeerProgrammingReport {
    pub rssi_dbm: i8,
    pub noise_floor_dbm: i8,
    pub link_metric: StaLinkMetric,
    pub ht_capabilities: Option<HtPeerCapabilities>,
    pub he_capabilities: Option<He20Capabilities>,
    pub he_peer_state: Option<He20PeerState>,
}

/// Complete result of the finite peer-programming transition.
#[derive(Debug, Eq, PartialEq)]
pub struct ProgrammedStaPeer {
    pub peer: ConnectedStaPeer,
    pub report: StaPeerProgrammingReport,
}

/// Exact policy or hardware edge which failed in the peer port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPeerPortError {
    ScanPolicy(EdcaParametersError),
    ScanWmm(EdcaParametersError),
    AssociationPlan(StaPeerAssociationPlanError),
    AssociationWmmMissing,
    AssociationWmm(EdcaParametersError),
    He20(He20InstallError),
    RateControl(MacHeBeamformingReportProfileError),
    /// The selected candidate carries no valid SSID.
    Ssid(oer_ieee80211_mac::ssid::WifiSsidError),
}

/// Stateless namespace for the two finite peer-policy transactions.
pub struct StaPeerPort;

impl StaPeerPort {
    /// Derive and install every policy needed before Authentication and
    /// Association can use the selected candidate.
    pub fn prepare<T: StaPeerTransmit>(
        transmit: &mut T,
        access_point: &ScanRecord,
    ) -> Result<PreparedStaPeer, StaPeerPortError> {
        let policy = StaPeerScanPolicy::new(access_point).map_err(StaPeerPortError::ScanPolicy)?;
        transmit.install_ht_ampdu_policy(policy.ht_ampdu);
        transmit.install_he_bss_color(policy.he_bss_color);
        transmit.install_bss_protection(policy.protection);
        if let Some(parameters) = policy.wmm.parameters() {
            transmit
                .install_wmm_edca(parameters)
                .map_err(StaPeerPortError::ScanWmm)?;
        }
        Ok(PreparedStaPeer {
            policy,
            access_point: *access_point,
        })
    }

    /// Complete and program one associated peer as a single driver-owned
    /// transition, then return the state needed by the connected runtime.
    pub fn program<H, T>(
        radio: StaPeerRadio<'_, H, T>,
        station: StaPeerStation,
        response: &AssociationResponse,
        prepared: PreparedStaPeer,
    ) -> Result<ProgrammedStaPeer, StaPeerPortError>
    where
        H: StaNoiseFloorHardware + He20PeerHardware + BeamformingReportHardware,
        T: StaPeerTransmit,
    {
        // Before any hardware write: the connected link probes the access
        // point with the SSID it associated with.
        let ssid = oer_ieee80211_mac::ssid::WifiSsid::new(prepared.access_point.ssid_bytes())
            .map_err(StaPeerPortError::Ssid)?;
        let noise_floor_dbm = radio.hardware.read_noise_floor_dbm();
        let plan = prepared
            .policy
            .complete(
                &prepared.access_point,
                response,
                station.association_phy,
                noise_floor_dbm,
            )
            .map_err(StaPeerPortError::AssociationPlan)?;

        radio.transmit.install_ht_ampdu_policy(plan.ht_ampdu);
        radio.transmit.install_he_bss_color(plan.he_bss_color);
        radio.transmit.install_bss_protection(plan.protection);
        if plan.wmm.source() == StaWmmSource::AssociationResponse {
            let parameters = plan
                .wmm
                .parameters()
                .ok_or(StaPeerPortError::AssociationWmmMissing)?;
            radio
                .transmit
                .install_wmm_edca(parameters)
                .map_err(StaPeerPortError::AssociationWmm)?;
        }
        if let Some(state) = plan.he_peer_state {
            // The complete hardware threshold-table builder is not reviewed.
            // Keep HE RTS disabled in MMIO and retain the finite threshold in
            // the TX runtime, which admits only a proven below-threshold
            // aggregate and rejects every other publication before DMA.
            let mut hardware_state = state;
            hardware_state.rts_threshold = None;
            program_he20_peer_state(
                radio.hardware,
                hardware_state,
                response.association_id,
                0,
                0,
            )
            .map_err(StaPeerPortError::He20)?;
        }
        plan.rate_control
            .program_hardware(radio.hardware)
            .map_err(StaPeerPortError::RateControl)?;
        oer_trace::emit(&oer_ieee80211_trace::LinkControlTrace {
            event: oer_ieee80211_trace::LinkEvent::Associated {
                association_id: response.association_id,
            },
        });

        let ht_capabilities = plan.ht_capabilities;
        let he_capabilities = plan.he_capabilities;
        let link = StaConnectedLink {
            station_address: station.station_address,
            bssid: prepared.access_point.bssid,
            ssid,
            association_id: response.association_id,
            beacon_interval_tu: prepared.access_point.beacon_interval_tu,
            beacon_timestamp_tsf: prepared.access_point.timestamp,
            beacon_received_at: prepared.access_point.received_at,
            peer_qos: plan.peer_qos,
            management_protection: station.management_protection,
            sae: station.sae,
            association_phy: station.association_phy,
            peer_supports_ht_short_guard_interval: match station.association_phy {
                PhyMode::Ht20 => prepared
                    .access_point
                    .supports_ht_short_guard_interval_20mhz(),
                PhyMode::Ht40 => prepared
                    .access_point
                    .supports_ht_short_guard_interval_40mhz(),
                PhyMode::Legacy | PhyMode::He20 => false,
            },
            peer_supports_ht_duplicate_mcs32: station.association_phy == PhyMode::Ht40
                && ht_capabilities.is_some_and(HtPeerCapabilities::supports_ht_duplicate_mcs32),
            peer_supports_one_ltf_800ns_gi: he_capabilities
                .is_some_and(|capability| capability.supports_one_ltf_800ns_gi()),
            peer_supports_ldpc: he_capabilities
                .is_some_and(|capability| capability.supports_ldpc_coding_in_payload()),
            peer_dcm_receive: he_capabilities
                .map_or(HeDcmConstellation::NotSupported, |capability| {
                    capability.dcm_receive_constellation()
                }),
        };
        Ok(ProgrammedStaPeer {
            peer: ConnectedStaPeer {
                link,
                rate_control: plan.rate_control,
            },
            report: StaPeerProgrammingReport {
                rssi_dbm: prepared.access_point.rssi,
                noise_floor_dbm,
                link_metric: plan.link_metric,
                ht_capabilities,
                he_capabilities,
                he_peer_state: plan.he_peer_state,
            },
        })
    }
}

#[cfg(test)]
mod tests;
