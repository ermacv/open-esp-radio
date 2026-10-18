#![expect(
    clippy::result_large_err,
    reason = "the test helper intentionally observes the concrete no-alloc preparation failure"
)]

use crate::{
    datapath::rx::{reorder::RxReorderCommandResources, staging::StagedRxQueue},
    roles::station::{
        control_mailbox::ConnectedControlResources, rx_protocol::AlwaysReadyConnectedRxSink,
    },
};

use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use oer_esp32s31_ieee80211_mac::tx::{
    HtDuplicateTxEvidenceGaps, HtDuplicateTxRejection, HtDuplicateTxUnavailable,
};
use oer_espressif_ieee80211_policy::rate_control::{
    HeLowMetricReportFeatures, StaLinkMetric, StaRateControlAssociation,
    StaRateControlAssociationInput, StaRateControlPhy,
};

use oer_esp32s31_ieee80211_sta::connected_rx::{
    ConnectedRxEvent, ConnectedRxSink, StaCcmpRxReplayEpoch, StaCcmpRxReplayResource,
};

use oer_ieee80211_softmac::{
    WifiConfig, WifiMacAddress, WifiMonitorConfig, WifiStationConfig,
    interface::{BoundVirtualInterface, ChannelContextId, VifId, VifRole, VirtualInterface},
};

use std::boxed::Box;

use super::*;

struct Sink;

impl ConnectedRxSink for Sink {
    fn publish(&mut self, _event: ConnectedRxEvent<'_>) {}
}

fn peer() -> ConnectedStaPeer {
    let link_metric = StaLinkMetric::from_rssi_and_noise_floor(-45, -95);
    ConnectedStaPeer {
        link: StaConnectedLink {
            station_address: [1, 2, 3, 4, 5, 6],
            bssid: [7, 8, 9, 10, 11, 12],
            ssid: oer_ieee80211_mac::ssid::WifiSsid::new(b"test").unwrap(),
            association_id: 7,
            beacon_interval_tu: 100,
            beacon_timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant::from_micros(0),
            beacon_received_at: None,
            peer_qos: true,
            management_protection: false,
            sae: false,
            association_phy: PhyMode::He20,
            peer_supports_ht_short_guard_interval: false,
            peer_supports_ht_duplicate_mcs32: false,
            peer_supports_one_ltf_800ns_gi: true,
            peer_supports_ldpc: true,
            peer_dcm_receive: HeDcmConstellation::Qam16,
        },
        rate_control: StaRateControlAssociation::new(StaRateControlAssociationInput {
            phy: StaRateControlPhy::He,
            link_metric,
            p2p: false,
            peer_highest_rate: None,
            long_range_rates_present: false,
            he_low_metric_report: HeLowMetricReportFeatures::default(),
        }),
    }
}

fn ht40_mcs32_peer() -> ConnectedStaPeer {
    let link_metric = StaLinkMetric::from_rssi_and_noise_floor(-45, -95);
    let mut peer = peer();
    peer.link.association_phy = PhyMode::Ht40;
    peer.link.peer_supports_ht_short_guard_interval = true;
    peer.link.peer_supports_ht_duplicate_mcs32 = true;
    peer.link.peer_supports_one_ltf_800ns_gi = false;
    peer.link.peer_supports_ldpc = false;
    peer.link.peer_dcm_receive = HeDcmConstellation::NotSupported;
    peer.rate_control = StaRateControlAssociation::new(StaRateControlAssociationInput {
        phy: StaRateControlPhy::Ht,
        link_metric,
        p2p: false,
        peer_highest_rate: None,
        long_range_rates_present: false,
        he_low_metric_report: HeLowMetricReportFeatures::default(),
    });
    peer
}

fn config() -> ConnectedStaConfig {
    ConnectedStaConfig {
        power: StationPowerMode::None,
        tx: ConnectedStaTxPolicy {
            rate: ConnectedStaRateConfig {
                high_throughput_enabled: true,
                fallback_legacy_rate: LegacyRate::Ofdm24M,
                fallback_ht_mcs: HtMcs::Mcs7,
                fallback_ht_guard_interval: HtGuardInterval::Long800Ns,
                ht_mcs_override: None,
                ht_guard_interval_override: None,
                he_mcs_override: None,
                he_guard_interval_and_ltf_override: None,
                he_dcm_override: None,
            },
            completion_timeout: oer_time::Duration::from_micros(250_000),
            aggregate_frame_limit: 32,
            aggregate_he_txop_limit: HeEdcaTxopLimit::DEFAULT,
            he_trigger_based: None,
        },
        block_ack: ConnectedStaBlockAckPolicy {
            tx_block_ack_window: 32,
            tx_block_ack_negotiation_timeout: oer_time::Duration::from_micros(500_000),
            tx_block_ack_negotiation_attempt_limit: 3,
            tid0_amsdu: false,
            rx_block_ack_maximum_window: 32,
            request_initial_tx_block_ack: true,
        },
        receive: ConnectedStaRxPolicy {
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            beacon_miss_limit: 10,
            beacon_timeout: oer_time::Duration::from_secs(6),
        },
    }
}

fn station_interface(peer: &ConnectedStaPeer) -> BoundVirtualInterface {
    BoundVirtualInterface::new(
        VirtualInterface::new(VifId::PRIMARY, VifRole::Station, peer.link.station_address),
        ChannelContextId::PRIMARY,
    )
}

fn prepare<const AGGREGATE_SLOTS: usize, const RX_REORDER_SLOTS: usize>(
    peer: ConnectedStaPeer,
    config: ConnectedStaConfig,
) -> Result<ConnectedStaPlan, ConnectedStaPrepareFailure> {
    let interface = station_interface(&peer);
    ConnectedStaPort::prepare_for_interface_with_storage::<AGGREGATE_SLOTS, RX_REORDER_SLOTS>(
        peer, config, interface,
    )
}

#[test]
fn plan_owns_rate_rx_tx_block_ack_and_beacon_policy() {
    let capabilities = ConnectedStaPort::capabilities();
    assert_eq!(capabilities.resources.channel_contexts, 1);
    assert!(capabilities.supports_rx_block_ack_window(32));
    assert!(capabilities.supports_tx_block_ack_window(32));

    let plan = prepare::<32, 32>(peer(), config()).unwrap();
    assert_eq!(plan.interface().interface.id, VifId::PRIMARY);
    assert_eq!(plan.interface().interface.role, VifRole::Station);
    assert_eq!(plan.interface().channel_context, ChannelContextId::PRIMARY);
    assert_eq!(plan.rx_config().association_id, 7);
    assert!(matches!(plan.data_tx_rate(), TxPhyRate::He(_)));
    assert!(matches!(plan.aggregate_tx_rate(), TxPhyRate::He(_)));
    assert_eq!(
        plan.ht_duplicate_tx_selection(),
        HtDuplicateTxSelection::NotRequested
    );
    assert_eq!(
        plan.beacon_loss().window(),
        oer_time::Duration::from_micros(6_000_000)
    );
}

#[test]
fn sta_mcs32_request_reaches_hardware_frontier_without_replacing_ordinary_rates() {
    let peer = ht40_mcs32_peer();
    let interface = station_interface(&peer);
    let request = HtDuplicateCertificationRequest::new(
        HtChannelWidth::Mhz40,
        HtGuardInterval::Short400Ns,
        5_484,
    );
    let plan = ConnectedStaPort::prepare_for_interface_with_storage_and_ht_duplicate_certification::<
        32,
        32,
    >(peer, config(), interface, Some(request))
    .unwrap();

    assert!(matches!(plan.data_tx_rate(), TxPhyRate::Ht(_)));
    assert!(matches!(plan.aggregate_tx_rate(), TxPhyRate::Ht(_)));
    let selection = plan.ht_duplicate_tx_selection();
    assert_eq!(selection.request(), Some(request));
    assert_eq!(selection.plan(), None);
    assert_eq!(
        selection.rejection(),
        Some(HtDuplicateTxRejection::Hardware(
            HtDuplicateTxUnavailable::Esp32s31EvidenceIncomplete(
                HtDuplicateTxEvidenceGaps::ESP32S31,
            )
        ))
    );
}

#[test]
fn ccmp_replay_plan_rejections_return_the_exact_rx_endpoint() {
    let open_peer = peer();
    let open_interface = station_interface(&open_peer);
    let mut open_plan =
        ConnectedStaPort::prepare_for_interface_with_storage_and_security::<32, 32>(
            open_peer,
            config(),
            open_interface,
            oer_ieee80211_mac::security::LinkProtection::Open,
        )
        .unwrap();
    let open_resource = Box::leak(Box::new(StaCcmpRxReplayResource::new()));
    let (open_rx, mut open_control) = open_resource
        .start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap())
        .unwrap();
    let (error, mut returned_open_rx) = open_plan
        .enable_ccmp_rx_replay(open_rx)
        .unwrap_err()
        .into_parts();
    assert_eq!(error, ConnectedStaCcmpReplayError::RequiresWpa2);
    returned_open_rx.stop().unwrap();
    open_control.stop().unwrap();

    let mut wpa2_plan = prepare::<32, 32>(peer(), config()).unwrap();
    let installed_resource = Box::leak(Box::new(StaCcmpRxReplayResource::new()));
    let (installed_rx, mut installed_control) = installed_resource
        .start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap())
        .unwrap();
    wpa2_plan.enable_ccmp_rx_replay(installed_rx).unwrap();

    let returned_resource = Box::leak(Box::new(StaCcmpRxReplayResource::new()));
    let (second_rx, mut second_control) = returned_resource
        .start(StaCcmpRxReplayEpoch::new([0; 8], 2, [0; 8]).unwrap())
        .unwrap();
    let (error, mut returned_second_rx) = wpa2_plan
        .enable_ccmp_rx_replay(second_rx)
        .unwrap_err()
        .into_parts();
    assert_eq!(error, ConnectedStaCcmpReplayError::AlreadyInstalled);
    returned_second_rx.stop().unwrap();
    second_control.stop().unwrap();

    let mut recovered_installed_rx = wpa2_plan
        .take_ccmp_rx_replay()
        .expect("first endpoint remains installed after rejecting the second");
    recovered_installed_rx.stop().unwrap();
    installed_control.stop().unwrap();
}

#[test]
fn port_binds_rx_and_control_to_one_validated_peer_plan() {
    let mut plan = prepare::<32, 32>(peer(), config()).unwrap();
    let reorder_storage = RxReorderFrameStorage::<128>::new();
    let queue: StagedRxQueue<'_, NoopRawMutex, 2, 128, 2> = StagedRxQueue::new();
    let (_, frames) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let reorder_commands = RxReorderCommandResources::<NoopRawMutex>::new();
    let (reorder_sender, reorder_receiver) = reorder_commands.split();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];
    let protocol_runtime = Box::leak(Box::new(ConnectedReceiveStorage::new()));
    let protocol = ConnectedStaPort::build_rx_protocol(
        &mut plan,
        ConnectedStaRxProtocolResources {
            frames,
            irq: &irq,
            sink: AlwaysReadyConnectedRxSink(Sink),
            mpdu: &mut mpdu,
            ethernet: &mut ethernet,
            reorder_commands: reorder_receiver,
            reorder_storage: &reorder_storage,
            runtime: protocol_runtime,
            reorder_scratch: None,
            clock: oer_time_virtual::SkipClock::new(),
            pipeline_observer: None,
            reorder_observer: None,
        },
    );
    assert_eq!(protocol.dispatcher().config(), plan.rx_config());

    let control_resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
    let (_, receiver) = control_resources.split();
    let rx_block_ack = StaApRxBlockAck::with_maximum_window(32).unwrap();
    let mut control_slot = crate::roles::station::control_slot::ConnectedControlSlot::new();
    let control = ConnectedStaPort::build_control(
        &plan,
        ConnectedStaControlResources {
            slot: &mut control_slot,
            receiver,
            reorder_commands: reorder_sender,
            rx_block_ack: &rx_block_ack,
            tsf_epoch: 1,
            power: None,
        },
    );
    assert_eq!(control.rx_block_ack().maximum_window(), 32);
    let beacon_binding = control
        .hardware_beacon_monitor_binding()
        .expect("connected composition binds one hardware-monitor admission epoch");
    assert_eq!(beacon_binding.bssid(), plan.link().bssid);
    assert_eq!(beacon_binding.association_id().get(), 7);
    assert_eq!(control.hardware_beacon_monitor_frontier(), None);
    assert_eq!(
        control
            .beacon_monitor()
            .expect("plan enables beacon loss")
            .config(),
        plan.beacon_loss()
    ); // The control was built in the slot; releasing it empties the slot for
    // the next association.
    let slot = control.release();
    assert!(!slot.is_occupied());
}

#[test]
fn invalid_config_returns_the_exact_peer_before_owner_handoff() {
    let original = peer();
    let link = original.link;
    let mut invalid = config();
    invalid.tx.aggregate_frame_limit = 33;
    let failure = prepare::<32, 32>(original, invalid).unwrap_err();
    assert_eq!(
        failure.error,
        ConnectedStaConfigError::AggregateFrameLimit {
            limit: 33,
            capacity: 32,
        }
    );
    assert_eq!(failure.peer.link, link);
}

#[test]
fn invalid_association_id_never_creates_a_beacon_monitor_epoch() {
    let mut invalid_peer = peer();
    invalid_peer.link.association_id = 0;
    let link = invalid_peer.link;
    let failure = prepare::<32, 32>(invalid_peer, config()).unwrap_err();
    assert_eq!(
        failure.error,
        ConnectedStaConfigError::InvalidAssociationId(0)
    );
    assert_eq!(failure.peer.link, link);
}

#[test]
fn zero_tx_block_ack_attempt_limit_is_rejected_before_owner_handoff() {
    let original = peer();
    let link = original.link;
    let mut invalid = config();
    invalid.block_ack.tx_block_ack_negotiation_attempt_limit = 0;
    let failure = prepare::<32, 32>(original, invalid).unwrap_err();
    assert_eq!(
        failure.error,
        ConnectedStaConfigError::ZeroTxBlockAckNegotiationAttemptLimit
    );
    assert_eq!(failure.peer.link, link);
}

#[test]
fn compact_profile_rejects_rx_window_larger_than_reorder_storage() {
    let original = peer();
    let link = original.link;
    let failure = prepare::<32, 8>(original, config()).unwrap_err();
    assert_eq!(
        failure.error,
        ConnectedStaConfigError::RxBlockAckWindowExceedsStorage {
            window: 32,
            capacity: 8,
        }
    );
    assert_eq!(failure.peer.link, link);
}

#[test]
fn explicit_vif_binding_rejects_unimplemented_role_before_owner_handoff() {
    let original = peer();
    let link = original.link;
    let interface = BoundVirtualInterface::new(
        VirtualInterface::new(VifId::new(1), VifRole::AccessPoint, link.station_address),
        ChannelContextId::PRIMARY,
    );
    let failure = ConnectedStaPort::prepare_for_interface_with_storage::<32, 32>(
        original,
        config(),
        interface,
    )
    .unwrap_err();
    assert_eq!(
        failure.error,
        ConnectedStaConfigError::InterfaceRole(VifRole::AccessPoint)
    );
    assert_eq!(failure.peer.link, link);
}

#[test]
fn application_wifi_plan_materializes_the_selected_station_vif() {
    let mut original = peer();
    original.link.station_address = [2, 2, 3, 4, 5, 6];
    let address = WifiMacAddress::new(original.link.station_address).unwrap();
    let wifi = WifiConfig::station(WifiStationConfig::new(address))
        .validate(ConnectedStaPort::capabilities())
        .unwrap();
    let plan =
        ConnectedStaPort::prepare_for_wifi_plan_with_storage::<32, 32>(original, config(), wifi)
            .unwrap();
    assert_eq!(plan.interface(), wifi.station().unwrap());
}

#[test]
fn wifi_plan_without_station_returns_the_exact_peer() {
    let original = peer();
    let link = original.link;
    let mut capabilities = ConnectedStaPort::capabilities();
    capabilities.interfaces.normalized_monitor_tap = true;
    let wifi = WifiConfig::monitor(WifiMonitorConfig::normalized())
        .validate(capabilities)
        .unwrap();
    let failure =
        ConnectedStaPort::prepare_for_wifi_plan_with_storage::<32, 32>(original, config(), wifi)
            .unwrap_err();
    assert_eq!(
        failure.error,
        ConnectedStaConfigError::MissingStationInterface
    );
    assert_eq!(failure.peer.link, link);
}
