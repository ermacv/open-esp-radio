const TEST_HT_CAPABILITIES: oer_ieee80211_mac::ht::HtLocalCapabilities =
    oer_ieee80211_mac::ht::HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01);

use oer_esp32s31_hal::types::{
    MacAssociationId, MacHe20PeerConfig, MacHe20PeerError, MacHeBeamformingReportProfile,
    MacHeErSuAckRateProfile, MacMinimumMpduStartSpacing,
};

use oer_esp32s31_ieee80211_mac::{
    rate::schedule::RateScheduleKind,
    tx::protection::{BssProtection, HePacketPadding, HeTxopDurationRtsThreshold},
};

use oer_ieee80211_mac::{
    channel::{WifiChannel, WifiChannelWidth},
    extensions::wmm::parse_wmm_parameter_element,
    ht::{HtDuplicateMcs32, ht_capability_ie, ht_operation_ie},
};

use super::*;

const HE20_MCS9_CAPABILITY: [u8; 24] = [
    255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0, 0x1f,
    0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
];
// HE TXOP Duration RTS Threshold 64: bits 9:4 live in byte four and bits
// 3:0 in the high nibble of byte three.
const HE20_OPERATION: [u8; 9] = [255, 7, 36, 0, 4, 0, 5, 0xfd, 0xff];
const STANDARD_WMM: [u8; 26] = [
    221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42, 0x43,
    94, 0, 0x72, 0x32, 47, 0,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Event {
    Ht,
    Color(u8),
    Wmm,
    Protection(BssProtection),
    HePeer(Option<u16>),
    HeAssociation(u16),
    BufferStatus,
    Beamforming,
    ErSuAck,
}

struct MockRadio {
    events: [Option<Event>; 16],
    count: usize,
    noise_floor_dbm: i8,
}

impl MockRadio {
    fn new(noise_floor_dbm: i8) -> Self {
        Self {
            events: [None; 16],
            count: 0,
            noise_floor_dbm,
        }
    }

    fn push(&mut self, event: Event) {
        self.events[self.count] = Some(event);
        self.count += 1;
    }
}

impl StaNoiseFloorHardware for MockRadio {
    fn read_noise_floor_dbm(&self) -> i8 {
        self.noise_floor_dbm
    }
}

impl He20PeerHardware for MockRadio {
    fn program_he20_peer(
        &mut self,
        _config: MacHe20PeerConfig,
        rts_threshold: Option<u16>,
    ) -> Result<(), MacHe20PeerError> {
        self.push(Event::HePeer(rts_threshold));
        Ok(())
    }

    fn program_he20_association(
        &mut self,
        association_id: MacAssociationId,
        _minimum_mpdu_start_spacing: MacMinimumMpduStartSpacing,
        _bssid_index: u8,
    ) {
        self.push(Event::HeAssociation(association_id.get() as u16));
    }

    fn initialize_he_buffer_status_report(&mut self) {
        self.push(Event::BufferStatus);
    }
}

impl BeamformingReportHardware for MockRadio {
    fn set_he_beamforming_report_profile(&mut self, _profile: MacHeBeamformingReportProfile) {
        self.push(Event::Beamforming);
    }

    fn set_he_ersu_ack_rate_profile(&mut self, _profile: MacHeErSuAckRateProfile) {
        self.push(Event::ErSuAck);
    }
}

struct MockTransmit {
    events: [Option<Event>; 8],
    count: usize,
}

impl MockTransmit {
    fn new() -> Self {
        Self {
            events: [None; 8],
            count: 0,
        }
    }

    fn push(&mut self, event: Event) {
        self.events[self.count] = Some(event);
        self.count += 1;
    }
}

impl StaPeerTransmit for MockTransmit {
    fn install_ht_ampdu_policy(&mut self, _parameters: HtPeerAmpduParameters) {
        self.push(Event::Ht);
    }

    fn install_he_bss_color(&mut self, bss_color: u8) {
        self.push(Event::Color(bss_color));
    }

    fn install_wmm_edca(
        &mut self,
        _parameters: WmmParameterSet,
    ) -> Result<(), EdcaParametersError> {
        self.push(Event::Wmm);
        Ok(())
    }

    fn install_bss_protection(&mut self, protection: BssProtection) {
        self.push(Event::Protection(protection));
    }
}

fn he20_access_point() -> ScanRecord {
    let mut access_point = ScanRecord::EMPTY;
    access_point.bssid = [2, 3, 4, 5, 6, 7];
    access_point.ssid[..4].copy_from_slice(b"test");
    access_point.ssid_len = 4;
    access_point.beacon_interval_tu = 100;
    access_point.rssi = -45;
    access_point.ht_capability_ie_present = true;
    access_point.ht_capability_ie[4] = 0x17;
    access_point.he_capability_ie[..HE20_MCS9_CAPABILITY.len()]
        .copy_from_slice(&HE20_MCS9_CAPABILITY);
    access_point.he_capability_ie_len = HE20_MCS9_CAPABILITY.len() as u8;
    access_point.he_operation_ie[..HE20_OPERATION.len()].copy_from_slice(&HE20_OPERATION);
    access_point.he_operation_ie_len = HE20_OPERATION.len() as u8;
    access_point
}

fn ht40_mcs32_access_point() -> ScanRecord {
    let channel = WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above).unwrap();
    let mut capability = ht_capability_ie(
        TEST_HT_CAPABILITIES,
        oer_ieee80211_mac::channel::Channel::from_wifi_channel(channel),
    );
    HtDuplicateMcs32::new().advertise_receive_only(&mut capability);
    let operation = ht_operation_ie(
        oer_ieee80211_mac::channel::Channel::from_wifi_channel(channel),
        oer_ieee80211_mac::protection::HtOperationProtection::default(),
    );

    let mut access_point = ScanRecord::EMPTY;
    access_point.bssid = [2, 3, 4, 5, 6, 7];
    access_point.ssid[..4].copy_from_slice(b"test");
    access_point.ssid_len = 4;
    access_point.channel = channel.primary();
    access_point.beacon_interval_tu = 100;
    access_point.rssi = -45;
    access_point.ht_capability_ie = capability;
    access_point.ht_capability_ie_present = true;
    access_point.ht_operation_ie = operation;
    access_point.ht_operation_ie_present = true;
    access_point
}

#[test]
fn port_owns_scan_and_association_peer_programming() {
    let access_point = he20_access_point();
    let mut transmit = MockTransmit::new();
    let prepared = StaPeerPort::prepare(&mut transmit, &access_point).unwrap();
    assert_eq!(
        &transmit.events[..transmit.count],
        &[
            Some(Event::Ht),
            Some(Event::Color(5)),
            Some(Event::Protection(BssProtection::UNPROTECTED)),
        ]
    );

    let response = AssociationResponse {
        capability_info: 0,
        status_code: 0,
        association_id: 7,
        ht_capability: true,
        he_capability: true,
        he_operation: true,
        wmm: true,
        wmm_parameters: Some(parse_wmm_parameter_element(&STANDARD_WMM).unwrap()),
        association_comeback_tu: None,
    };
    let mut hardware = MockRadio::new(-95);
    let programmed = StaPeerPort::program(
        StaPeerRadio::new(&mut hardware, &mut transmit),
        StaPeerStation::new([8, 9, 10, 11, 12, 13], PhyMode::He20, false, false),
        &response,
        prepared,
    )
    .unwrap();

    assert_eq!(programmed.peer.link.bssid, access_point.bssid);
    assert_eq!(programmed.peer.link.ssid.as_bytes(), b"test");
    assert_eq!(programmed.peer.link.association_id, 7);
    let threshold = HeTxopDurationRtsThreshold::new(64).unwrap();
    assert_eq!(
        programmed
            .report
            .he_peer_state
            .and_then(|state| state.rts_threshold),
        Some(64)
    );
    assert!(!programmed.peer.link.peer_supports_ht_short_guard_interval);
    assert_eq!(programmed.report.link_metric.value(), 50);
    assert_eq!(
        programmed.peer.rate_control.current_schedule().kind,
        RateScheduleKind::Dot11Ax
    );
    assert_eq!(
        &transmit.events[..transmit.count],
        &[
            Some(Event::Ht),
            Some(Event::Color(5)),
            Some(Event::Protection(BssProtection::UNPROTECTED)),
            Some(Event::Ht),
            Some(Event::Color(5)),
            // HE PHY Capabilities without PPE Thresholds advertise Nominal
            // Packet Padding code two (16 us) in element byte 18.
            Some(Event::Protection(BssProtection {
                he_txop_rts_threshold: Some(threshold),
                he_packet_padding: HePacketPadding::Us16,
                ..BssProtection::UNPROTECTED
            })),
            Some(Event::Wmm),
        ]
    );
    assert_eq!(
        &hardware.events[..hardware.count],
        &[
            Some(Event::HePeer(None)),
            Some(Event::HeAssociation(7)),
            Some(Event::BufferStatus),
            Some(Event::Beamforming),
            Some(Event::ErSuAck),
        ]
    );
}

#[test]
fn scan_mcs32_capability_reaches_the_connected_ht40_owner_without_tx_admission() {
    let access_point = ht40_mcs32_access_point();
    assert!(access_point.supports_ht_duplicate_mcs32());
    let mut transmit = MockTransmit::new();
    let prepared = StaPeerPort::prepare(&mut transmit, &access_point).unwrap();
    assert!(
        prepared
            .policy
            .ht_capabilities
            .is_some_and(HtPeerCapabilities::supports_ht_duplicate_mcs32)
    );

    let response = AssociationResponse {
        capability_info: 0,
        status_code: 0,
        association_id: 7,
        ht_capability: true,
        he_capability: false,
        he_operation: false,
        wmm: false,
        wmm_parameters: None,
        association_comeback_tu: None,
    };
    let mut hardware = MockRadio::new(-95);
    let programmed = StaPeerPort::program(
        StaPeerRadio::new(&mut hardware, &mut transmit),
        StaPeerStation::new([8, 9, 10, 11, 12, 13], PhyMode::Ht40, false, false),
        &response,
        prepared,
    )
    .unwrap();

    assert_eq!(programmed.peer.link.association_phy, PhyMode::Ht40);
    assert!(programmed.peer.link.peer_supports_ht_short_guard_interval);
    assert!(programmed.peer.link.peer_supports_ht_duplicate_mcs32);
    assert!(
        programmed
            .report
            .ht_capabilities
            .is_some_and(HtPeerCapabilities::supports_ht_duplicate_mcs32)
    );
    assert_eq!(
        programmed.peer.rate_control.current_schedule().kind,
        RateScheduleKind::Dot11N
    );
}

#[test]
fn a_candidate_without_an_ssid_is_refused_before_any_peer_write() {
    let mut access_point = he20_access_point();
    access_point.ssid_len = 0;
    let mut transmit = MockTransmit::new();
    let prepared = StaPeerPort::prepare(&mut transmit, &access_point).unwrap();
    let prepared_events = transmit.count;
    let response = AssociationResponse {
        capability_info: 0,
        status_code: 0,
        association_id: 7,
        ht_capability: true,
        he_capability: true,
        he_operation: true,
        wmm: true,
        wmm_parameters: Some(parse_wmm_parameter_element(&STANDARD_WMM).unwrap()),
        association_comeback_tu: None,
    };
    let mut hardware = MockRadio::new(-95);
    let refused = StaPeerPort::program(
        StaPeerRadio::new(&mut hardware, &mut transmit),
        StaPeerStation::new([8, 9, 10, 11, 12, 13], PhyMode::He20, false, false),
        &response,
        prepared,
    );
    assert!(matches!(
        refused,
        Err(StaPeerPortError::Ssid(
            oer_ieee80211_mac::ssid::WifiSsidError::Empty
        ))
    ));
    assert_eq!(transmit.count, prepared_events);
}
