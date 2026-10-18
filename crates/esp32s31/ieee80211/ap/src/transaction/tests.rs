use core::pin::pin;
use oer_time_virtual::SkipClock;

use oer_esp32s31_hal::types::{
    MacKeyInstallOutcome, MacLegacyRate, MacLegacyTxProgram, MacTxCompletionObservation,
    MacTxDetachOutcome, MacTxDetachReason, MacTxProtection, MacTxQueueDetached,
};

use oer_esp32s31_ieee80211::ordinary_tx::WifiTxPowerPair;

use oer_esp32s31_ieee80211_mac::{
    ap_policy::ApRxPolicyHardware,
    crypto::CcmpKeyHardware,
    tx::{HardwareOwnedTxDma, PreparedTxDma, TxSlot, runtime::WifiTxRuntimePolicy},
};

use oer_ieee80211_mac::{
    ap::ApAssociationSecurityObservation, beacon::AP_BEACON_CAPACITY, channel::WifiChannel,
    ssid::WifiSsid,
};

use oer_ieee80211_ap::{AccessPointService, ApAssociationCapabilities};
use oer_ieee80211_mac::protection::{ErpProtection, HtProtectionMode};

use oer_ieee80211_rsn::{Pmk, frames::RsnGtk};

use super::*;

#[derive(Default)]
struct Hardware {
    completion: Option<MacTxCompletionObservation>,
    publications: u8,
    legacy: Option<MacLegacyTxProgram>,
}

impl ApRxPolicyHardware for Hardware {
    fn apply_ap_link_policy(&mut self, _address: [u8; 6]) {}

    fn disable_ap_link_policy(&mut self) {}
}

impl CcmpKeyHardware for Hardware {
    fn install_sta_ccmp_entry(
        &mut self,
        _index: u8,
        _identity: oer_esp32s31_hal::types::MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, _index: u8) {}
}

impl oer_esp32s31_ieee80211_mac::ap_tsf::ApTsfHardware for Hardware {
    fn reset_and_start_access_point_tsf(
        &mut self,
        _: oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsfWrite,
    ) {
    }

    fn stop_access_point_tsf(
        &mut self,
        _: oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsfWrite,
    ) {
    }
}

impl TxHardware for Hardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        self.legacy = Some(program);
        true
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {
        self.publications = self.publications.saturating_add(1);
    }

    fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionObservation> {
        self.completion.take()
    }

    fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
        false
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _queue: u8,
        descriptor: u32,
        reason: MacTxDetachReason,
        detached: impl for<'a> FnOnce(MacTxQueueDetached<'a>) -> R,
    ) -> MacTxDetachOutcome<R> {
        match reason {
            MacTxDetachReason::Completed => {
                MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(descriptor)))
            }
            _ => MacTxDetachOutcome::NoEvent,
        }
    }

    fn take_block_ack_completion(
        &mut self,
        _queue: u8,
    ) -> Option<oer_esp32s31_hal::types::MacHtAmpduCompletionObservation> {
        None
    }
}

#[derive(Clone, Copy)]
struct Power;

impl WifiTxPowerProfile for Power {
    fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
        WifiTxPowerPair {
            primary: 1,
            alternate: 1,
        }
    }
}

#[test]
fn unschedulable_beacon_reports_preparation_failure_without_publication() {
    let ap = [2, 0, 0, 0, 0, 1];
    let mut hardware = Hardware::default();
    let mut beacon = [0; AP_BEACON_CAPACITY];
    let mut peers = oer_ieee80211_ap::AccessPointPeerStorage::new();
    let mut pairwise = crate::security::ApPairwiseKeyStorage::new();
    // A zero beacon interval has no next TBTT, so the retained beacon cannot
    // be stamped for publication.
    let engine = ApEngine::start(
        &mut hardware,
        AccessPointService::new(
            ap,
            Pmk::derive(b"password", b"ap").unwrap(),
            RsnGtk::new(1, true, [7; 16]).unwrap(),
            oer_ieee80211_ap::AccessPointClientLimit::new(2).unwrap(),
            oer_ieee80211_ap::AccessPointInactiveTimeout::default(),
            &mut peers,
        ),
        &mut beacon,
        &mut pairwise,
        &WifiSsid::new(b"ap").unwrap(),
        oer_ieee80211_mac::channel::WifiChannel::mhz20(6).unwrap(),
        0,
        2,
        1,
    )
    .unwrap_or_else(|_| panic!("AP engine starts"));
    let mut slot = pin!(TxSlot::<512>::new_model());
    let mut mac = ApMac::new(
        engine,
        WifiTxResources {
            slot: slot.as_mut(),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: || 0,
            timer: SkipClock::new(),
        },
        ApTxConfig {
            publication_timeout: oer_time::Duration::from_micros(1_000),
        },
    );

    assert_eq!(
        mac.publish_beacon(&mut hardware, oer_time::Instant::from_micros(102_400)),
        Err(ApMacError::Engine(ApEngineError::BeaconPreparation))
    );
    assert_eq!(mac.observation().beacons_transmitted, 0);
}

#[test]
fn prepared_beacon_becomes_evidence_only_after_terminal_success() {
    let ap = [2, 0, 0, 0, 0, 1];
    let mut hardware = Hardware::default();
    let mut beacon = [0; AP_BEACON_CAPACITY];
    let mut peers = oer_ieee80211_ap::AccessPointPeerStorage::new();
    let mut pairwise = crate::security::ApPairwiseKeyStorage::new();
    let engine = ApEngine::start(
        &mut hardware,
        AccessPointService::new(
            ap,
            Pmk::derive(b"password", b"ap").unwrap(),
            RsnGtk::new(1, true, [7; 16]).unwrap(),
            oer_ieee80211_ap::AccessPointClientLimit::new(2).unwrap(),
            oer_ieee80211_ap::AccessPointInactiveTimeout::default(),
            &mut peers,
        ),
        &mut beacon,
        &mut pairwise,
        &WifiSsid::new(b"ap").unwrap(),
        oer_ieee80211_mac::channel::WifiChannel::mhz20(6).unwrap(),
        100,
        2,
        1,
    )
    .unwrap_or_else(|_| panic!("AP engine starts"));
    let mut slot = pin!(TxSlot::<512>::new_model());
    let mut mac = ApMac::new(
        engine,
        WifiTxResources {
            slot: slot.as_mut(),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: || 0,
            timer: SkipClock::new(),
        },
        ApTxConfig {
            publication_timeout: oer_time::Duration::from_micros(1_000),
        },
    );

    mac.publish_beacon(&mut hardware, oer_time::Instant::from_micros(102_400))
        .unwrap();
    assert_eq!(mac.observation().beacons_transmitted, 0);
    hardware.completion = Some(MacTxCompletionObservation::new_model(0, 0));
    let (progress, action) = mac
        .service_tx(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: oer_esp32s31_ieee80211_mac::irq::EVENT_TX_COMPLETE,
            },
            oer_time::Instant::from_micros(1),
        )
        .unwrap();
    assert_eq!(progress, WifiTxProgress::Complete);
    assert_eq!(action, ApTxCompletionAction::None);
    assert_eq!(mac.observation().beacons_transmitted, 1);
    assert!(mac.try_into_parts().is_ok());
}

#[test]
fn mixed_bss_protects_ordinary_data_with_cts_to_self_at_a_dsss_rate() {
    let ap = [2, 0, 0, 0, 0, 1];
    let target = [2, 0, 0, 0, 0, 2];
    let legacy = [2, 0, 0, 0, 0, 3];
    let mut hardware = Hardware::default();
    let mut beacon = [0; AP_BEACON_CAPACITY];
    let mut peers = oer_ieee80211_ap::AccessPointPeerStorage::new();
    let mut pairwise = crate::security::ApPairwiseKeyStorage::new();
    let mut service = AccessPointService::new_open(
        ap,
        oer_ieee80211_ap::AccessPointClientLimit::new(2).unwrap(),
        oer_ieee80211_ap::AccessPointInactiveTimeout::default(),
        &mut peers,
    );
    let open = ApAssociationSecurityObservation {
        privacy: false,
        rsn_ie: None,
        rsn_ie_count: 0,
        rsnxe: None,
        rsnxe_count: 0,
        legacy_wpa_present: false,
        malformed_elements: false,
    };
    service.authenticate_open(target, oer_time::Instant::from_micros(1));
    let ht_ie = oer_ieee80211_mac::ht::ht_capability_ie(
        crate::profile::HT_CAPABILITIES,
        oer_ieee80211_mac::channel::Channel::from_wifi_channel(WifiChannel::mhz20(6).unwrap()),
    );
    service
        .associate_open(
            target,
            open,
            ApAssociationCapabilities {
                maximum_legacy_rate_500kbps: 108,
                short_preamble: true,
                ht: oer_ieee80211_mac::ht::ht_peer_capabilities(&ht_ie),
                qos_supported: true,
            },
            oer_time::Instant::from_micros(2),
        )
        .unwrap();
    service.authenticate_open(legacy, oer_time::Instant::from_micros(3));
    service
        .associate_open(
            legacy,
            open,
            ApAssociationCapabilities {
                maximum_legacy_rate_500kbps: 22,
                short_preamble: true,
                ht: None,
                qos_supported: false,
            },
            oer_time::Instant::from_micros(4),
        )
        .unwrap();
    let engine = ApEngine::start(
        &mut hardware,
        service,
        &mut beacon,
        &mut pairwise,
        &WifiSsid::new(b"ap").unwrap(),
        WifiChannel::mhz20(6).unwrap(),
        100,
        2,
        1,
    )
    .unwrap_or_else(|_| panic!("Open mixed AP starts"));
    assert_eq!(engine.bss_protection().erp, ErpProtection::new(true, false));
    assert_eq!(engine.bss_protection().ht, HtProtectionMode::NonHtMixed);

    let mut slot = pin!(TxSlot::<512>::new_model());
    let mut mac = ApMac::new(
        engine,
        WifiTxResources {
            slot: slot.as_mut(),
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy: || 0,
            timer: SkipClock::new(),
        },
        ApTxConfig {
            publication_timeout: oer_time::Duration::from_micros(1_000),
        },
    );
    let mut first = [0_u8; 18];
    first[..6].copy_from_slice(&target);
    first[6..12].copy_from_slice(&ap);
    first[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
    first[14..].copy_from_slice(&[1, 2, 3, 4]);
    let mut second = first;
    second[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 7]);
    second[14..].copy_from_slice(&[5, 6, 7, 8]);
    let data_sequence = mac.engine.current_data_sequence();
    let qos_sequence = mac.engine.current_qos_sequence(target, 0);
    let mut scratch = [0_u8; 256];

    mac.publish_ethernet(&mut hardware, target, &first, &mut scratch)
        .unwrap();
    assert_eq!(hardware.publications, 1);
    let control = hardware.legacy.unwrap().control();
    assert_eq!(control.protection, MacTxProtection::CtsToSelf);
    assert_eq!(control.rate, MacLegacyRate::Cck11MShort);
    assert_ne!(mac.engine.current_data_sequence(), data_sequence);
    assert_eq!(mac.engine.current_qos_sequence(target, 0), qos_sequence);
    assert!(mac.tx_pending());
}

mod probe;
