//! Shared production-boundary fixtures for AP owner tests.

use oer_esp32s31_hal::types::{MacCcmpKeyIdentity, MacKeyInstallOutcome};

use oer_esp32s31_ieee80211_mac::{
    ap_policy::ApRxPolicyHardware, ap_tsf::ApTsfHardware, crypto::CcmpKeyHardware,
};

use std::boxed::Box;

use xarxa_driver::{PacketBuf, PacketBufAllocator, PacketPool, PacketPoolStorage};

pub(super) struct Hardware;
impl ApRxPolicyHardware for Hardware {
    fn apply_ap_link_policy(&mut self, _: [u8; 6]) {}
    fn disable_ap_link_policy(&mut self) {}
}
impl ApTsfHardware for Hardware {
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
impl CcmpKeyHardware for Hardware {
    fn install_sta_ccmp_entry(
        &mut self,
        _: u8,
        _: MacCcmpKeyIdentity,
        _: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        panic!("open AP does not install keys")
    }
    fn clear_ccmp_entry(&mut self, _: u8) {}
}

pub(super) fn allocator<const N: usize>() -> PacketBufAllocator {
    let storage = Box::leak(Box::new(PacketPoolStorage::<N>::new()));
    Box::leak(Box::new(PacketPool::new(storage))).allocator()
}

pub(super) fn packet(pool: PacketBufAllocator, destination: u8, sequence: u8) -> PacketBuf {
    let mut packet = pool.try_alloc().unwrap();
    packet.set_len(15);
    packet.fill(0);
    packet[..6].fill(destination);
    packet[14] = sequence;
    packet
}

pub(super) fn with_authorized_ap(test: impl FnOnce(&mut super::super::ApEngine<'_>)) {
    with_authorized_ap_owner(|mut engine| test(&mut engine));
}

pub(super) fn with_authorized_ap_owner(test: impl FnOnce(super::super::ApEngine<'_>)) {
    with_authorized_ap_capabilities(false, test);
}

pub(super) fn with_authorized_ap_capabilities(
    ht: bool,
    test: impl FnOnce(super::super::ApEngine<'_>),
) {
    use oer_esp32s31_ieee80211_ap::{protocol::*, security::ApPairwiseKeyStorage};

    use oer_ieee80211_mac::{
        ap::ApAssociationSecurityObservation, beacon::AP_BEACON_CAPACITY, channel::WifiChannel,
        ssid::WifiSsid,
    };
    let mut peers = AccessPointPeerStorage::new();
    let mut service = AccessPointService::new_open(
        [2; 6],
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::default(),
        &mut peers,
    );
    let ht_ie = oer_ieee80211_mac::ht::ht_capability_ie(
        oer_esp32s31_ieee80211_ap::profile::HT_CAPABILITIES,
        oer_ieee80211_mac::channel::Channel::from_wifi_channel(WifiChannel::mhz20(13).unwrap()),
    );
    for peer in [[4; 6], [6; 6]] {
        service.authenticate_open(peer, oer_time::Instant::from_micros(0));
        service
            .associate_open(
                peer,
                ApAssociationSecurityObservation {
                    privacy: false,
                    rsn_ie: None,
                    rsn_ie_count: 0,
                    rsnxe: None,
                    rsnxe_count: 0,
                    legacy_wpa_present: false,
                    malformed_elements: false,
                },
                ApAssociationCapabilities {
                    maximum_legacy_rate_500kbps: 108,
                    short_preamble: true,
                    ht: ht.then(|| oer_ieee80211_mac::ht::ht_peer_capabilities(&ht_ie).unwrap()),
                    qos_supported: ht,
                },
                oer_time::Instant::from_micros(1),
            )
            .unwrap();
    }
    let mut beacon = [0; AP_BEACON_CAPACITY];
    let mut keys = ApPairwiseKeyStorage::new();
    let engine = super::super::ApEngine::start(
        &mut Hardware,
        service,
        &mut beacon,
        &mut keys,
        &WifiSsid::new(b"selection-test").unwrap(),
        WifiChannel::mhz20(13).unwrap(),
        100,
        2,
        1,
    )
    .unwrap_or_else(|_| panic!("open AP startup"));
    test(engine);
}
