use core::pin::Pin;
use oer_ieee80211_mac::block_ack::TX_BLOCK_ACK_MAX_TIDS;
use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_time::{Clock as _, Timer as _};

use crate::{
    datapath::{
        DatapathControlProgress, WifiTxProgress, WifiTxWake,
        rx::reorder::{
            RxReorderCommand, RxReorderCommandResources, try_receive_rx_reorder_command,
        },
    },
    roles::station::control_mailbox::{ConnectedControlResources, ConnectedSecurityFrame},
};

use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use oer_esp32s31_hal::types::{
    MacKeyInstallOutcome, MacLegacyTxProgram, MacStaReceivePolicySnapshot,
    MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason, MacTxQueueDetached,
};

use oer_esp32s31_ieee80211_mac::{
    MacInterface,
    crypto::{
        CcmpKeyHardware, CryptoKeyError, StaGroupCcmpKeyMaterial, StaGroupCcmpReplaceError,
        StaGroupCcmpSlot, install_sta_group_ccmp, install_sta_pairwise_ccmp,
        replace_sta_group_ccmp_with_rollback,
    },
    rx::{
        ampdu::{RxBlockAckRequest, RxBlockAckSnapshot},
        hardware::{RxBlockAckHardware, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    },
    tx::{
        HardwareOwnedTxDma, PreparedTxDma, TxHardware, TxSlot, ampdu::BlockAckAction,
        runtime::WifiTxRuntimePolicy,
    },
};

use oer_esp32s31_ieee80211_sta::{
    connected_rx::{
        ConnectedRxEvent, ConnectedRxSink, StaCcmpRxReplayEpoch, StaCcmpRxReplayResource,
        StaCcmpRxReplayRxEndpoint,
    },
    single_mpdu_tx::{
        SingleMpduTx, SingleMpduTxConfig, SingleMpduTxOutcome, WifiTxPowerPair, WifiTxPowerProfile,
    },
};

use oer_ieee80211_mac::{
    qos::WmmAccessCategory,
    station::{StaDisconnect, StaDisconnectKind, StaTxSequenceCounters},
    station_beacon::{StaBeaconObservation, StaBeaconProtection, StaTimObservation},
    twt::{
        IndividualTwtAction, IndividualTwtControl, IndividualTwtFlowId, IndividualTwtFlowType,
        IndividualTwtParameterSet, IndividualTwtSetup, IndividualTwtSetupCommand,
    },
};

use oer_ieee80211_softmac::MacRxMetadata;

use oer_ieee80211_rsn::{
    OwnedEapolFrame, Pmk, Ptk, PtkContext, RsnInterface,
    aes::{RsnSoftwareAes, software_aes128_key_wrap},
    frames::{OwnedRsnIe, RsnGtk, RsnPlainKeyData, RsnTxFrame},
    supplicant::{RsnConnectedSupplicant, RsnStaSupplicant, RsnStaSupplicantAction},
};
use oer_ieee80211_rsn_service::supplicant::process_frame;

use oer_esp32s31_ieee80211_sta::{
    connected_control::{ConnectedPowerCommand, PowerCoexSnapshot},
    modem_sleep::{CoexPhaseView, CoexView, PmCoexAction, PmCoexEvent, PmState},
};

use super::*;

const STATION: [u8; 6] = [2, 3, 4, 5, 6, 7];
const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
const SNONCE: [u8; 32] = [0x33; 32];
const ANONCE: [u8; 32] = [0x44; 32];
const RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];
const INITIAL_GTK: [u8; 16] = [0x11; 16];

use oer_espressif_ieee80211_policy::block_ack::STA_TX_BLOCK_ACK_TIDS;

/// The station's originator as the connected plan builds it.
fn tx_block_ack() -> oer_ieee80211_mac::block_ack::TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS> {
    oer_ieee80211_mac::block_ack::TxBlockAckOriginator::new(
        oer_ieee80211_mac::block_ack::TxBlockAckOriginatorPolicy {
            tids: &STA_TX_BLOCK_ACK_TIDS,
            first_dialog_token: oer_espressif_ieee80211_policy::block_ack::FIRST_DIALOG_TOKEN,
            next_dialog_token: oer_espressif_ieee80211_policy::block_ack::next_dialog_token,
        },
        oer_ieee80211_mac::block_ack::TxBlockAckOriginatorConfig {
            window: 32,
            negotiation_timeout: oer_time::Duration::from_micros(100_000),
            amsdu_tids: 1,
        },
    )
    .unwrap()
}

fn ptk_context() -> PtkContext {
    PtkContext {
        authenticator_address: BSSID,
        supplicant_address: STATION,
        authenticator_nonce: ANONCE,
        supplicant_nonce: SNONCE,
    }
}

fn owned_eapol(frame: &RsnTxFrame<512>) -> OwnedEapolFrame<512> {
    OwnedEapolFrame::try_copy(RsnInterface::Station, BSSID, frame.as_bytes()).unwrap()
}

fn established_supplicant() -> (RsnConnectedSupplicant, Ptk) {
    let pmk = Pmk::from_bytes([0x22; 32]);
    let peer_ptk = pmk.derive_ptk(oer_ieee80211_mac::security::rsn::Akm::Psk, ptk_context());
    let mut supplicant =
        RsnStaSupplicant::try_new(STATION, BSSID, SNONCE, &RSN, &RSN, &[]).unwrap();
    let mut aes = RsnSoftwareAes::new();

    let message1 = RsnTxFrame::<512>::message1(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        STATION,
        1,
        ANONCE,
    )
    .unwrap();
    assert!(matches!(
        embassy_futures::block_on(process_frame(
            &mut supplicant,
            owned_eapol(&message1),
            &pmk,
            &mut aes
        )),
        Ok(RsnStaSupplicantAction::Transmit(_))
    ));

    let rsn = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let gtk = RsnGtk::new(1, false, INITIAL_GTK).unwrap();
    let plain = RsnPlainKeyData::<64>::build(rsn.as_bytes(), &gtk, None).unwrap();
    let wrapped = software_aes128_key_wrap(peer_ptk.kek(), plain.as_bytes()).unwrap();
    let message3 = RsnTxFrame::<512>::message3(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        STATION,
        2,
        ANONCE,
        [5, 0, 0, 0, 0, 0, 0, 0],
        wrapped.as_bytes(),
    )
    .unwrap()
    .authenticate(&peer_ptk);
    let RsnStaSupplicantAction::InstallKeys(request) = embassy_futures::block_on(process_frame(
        &mut supplicant,
        owned_eapol(&message3),
        &pmk,
        &mut aes,
    ))
    .unwrap() else {
        panic!("authenticated Message 3 must establish connected WPA2 state")
    };
    assert!(matches!(
        supplicant.complete_key_install::<512>(request, true),
        Ok(RsnStaSupplicantAction::Transmit(_))
    ));
    (supplicant.into_connected().unwrap(), peer_ptk)
}

fn group_message1(
    ptk: &Ptk,
    replay_counter: u64,
    key_id: u8,
    key: [u8; 16],
    receive_sequence: [u8; 8],
) -> ConnectedSecurityFrame {
    let mut kde = [0; 24];
    kde[..8].copy_from_slice(&[0xdd, 22, 0, 0x0f, 0xac, 1, key_id, 0]);
    kde[8..].copy_from_slice(&key);
    let wrapped = software_aes128_key_wrap(ptk.kek(), &kde).unwrap();
    let frame = RsnTxFrame::<512>::group_message1(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        STATION,
        replay_counter,
        receive_sequence,
        wrapped.as_bytes(),
    )
    .unwrap()
    .authenticate(ptk);
    ConnectedSecurityFrame::Protected(owned_eapol(&frame))
}

#[derive(Default)]
struct Hardware {
    station_tsf: u64,
    station_tsf_writes: usize,
    mac_local_time: u32,
    prepare: bool,
    completion: Option<MacTxCompletionObservation>,
    programmed: Option<S31RxBlockAckAgreement>,
    cleared: [Option<u8>; 4],
    clear_count: usize,
    he_tid: [Option<(u8, bool)>; 4],
    he_count: usize,
    key_install_count: usize,
    ccmp_reject_installs: u8,
    ccmp_clears: usize,
    twt_admit: bool,
    twt_install_count: usize,
    station_policy: Option<MacStaReceivePolicySnapshot>,
    tbtt: Option<oer_esp32s31_hal::types::StaTbttSchedule>,
    tx_blocked: bool,
    beacon_pti: Option<oer_esp32s31_hal::types::MacPti>,
}

impl CcmpKeyHardware for Hardware {
    fn install_sta_ccmp_entry(
        &mut self,
        _index: u8,
        _identity: oer_esp32s31_hal::types::MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        if self.ccmp_reject_installs != 0 {
            self.ccmp_reject_installs -= 1;
            return MacKeyInstallOutcome::Rejected;
        }
        self.key_install_count += 1;
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, _index: u8) {
        self.ccmp_clears += 1;
    }
}

impl TxHardware for Hardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        _program: MacLegacyTxProgram,
    ) -> bool {
        self.prepare
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {}

    fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionObservation> {
        self.completion.take()
    }

    fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
        false
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        match reason {
            MacTxDetachReason::Completed => MacTxDetachOutcome::Detached(detached(
                MacTxQueueDetached::new_model(expected_descriptor_head),
            )),
            MacTxDetachReason::Collision | MacTxDetachReason::Timeout => {
                MacTxDetachOutcome::NoEvent
            }
        }
    }

    fn take_block_ack_completion(
        &mut self,
        _queue: u8,
    ) -> Option<oer_esp32s31_hal::types::MacHtAmpduCompletionObservation> {
        None
    }
}

impl RxBlockAckHardware for Hardware {
    fn program_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        self.programmed = Some(agreement.validate()?);
        Ok(())
    }

    fn clear_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        if hardware_index >= 8 {
            return Err(S31RxBlockAckAgreementError::HardwareIndex(hardware_index));
        }
        self.cleared[self.clear_count] = Some(hardware_index);
        self.clear_count += 1;
        Ok(())
    }

    fn reset_rx_block_ack_window(
        &mut self,
        hardware_index: u8,
        tid: u8,
        _starting_sequence: SequenceNumber,
        window: u16,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        if hardware_index >= 8 {
            return Err(S31RxBlockAckAgreementError::HardwareIndex(hardware_index));
        }
        if tid > 7 {
            return Err(S31RxBlockAckAgreementError::Tid(tid));
        }
        if window == 0 || window > 0x7f {
            return Err(S31RxBlockAckAgreementError::Window(window));
        }
        Ok(())
    }

    fn program_extra_softap_rx_block_ack(
        &mut self,
        agreement: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        self.programmed = Some(agreement.validate()?);
        Ok(())
    }

    fn clear_extra_softap_rx_block_ack(
        &mut self,
        hardware_index: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        self.clear_rx_block_ack(hardware_index)
    }

    fn reset_extra_softap_rx_block_ack_window(
        &mut self,
        hardware_index: u8,
        _starting_sequence: SequenceNumber,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        if hardware_index >= 8 {
            return Err(S31RxBlockAckAgreementError::HardwareIndex(hardware_index));
        }
        Ok(())
    }
}

impl oer_esp32s31_ieee80211::station_tsf::StationTsfHardware for Hardware {
    fn station_tsf(&mut self) -> u64 {
        self.station_tsf
    }

    fn set_station_tsf(
        &mut self,
        _: oer_esp32s31_ieee80211::station_tsf::StationTsfWrite,
        value: u64,
    ) {
        self.station_tsf = value;
        self.station_tsf_writes += 1;
    }
}

impl ConnectedControlHardware for Hardware {
    fn disable_station_receive_policy(&mut self) {}

    fn mac_local_time(&mut self) -> u32 {
        self.mac_local_time
    }

    fn station_beacon_monitor_readback(&mut self) -> Option<MacStaReceivePolicySnapshot> {
        self.station_policy
    }

    fn set_he_tid_enabled(
        &mut self,
        tid: u8,
        enabled: bool,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        if tid >= 8 {
            return Err(S31RxBlockAckAgreementError::Tid(tid));
        }
        self.he_tid[self.he_count] = Some((tid, enabled));
        self.he_count += 1;
        Ok(())
    }

    fn admit_station_individual_twt(
        &mut self,
        _proposal: &IndividualTwtProposal,
    ) -> Result<(), StationIndividualTwtHardwareError> {
        if self.twt_admit {
            Ok(())
        } else {
            Err(StationIndividualTwtHardwareError::Unsupported {
                reached: StationIndividualTwtHardwareStage::None,
                missing: StationIndividualTwtUnsupportedStage::ItwtCoexistenceAdmission,
            })
        }
    }

    fn install_station_individual_twt(
        &mut self,
        _agreement: &oer_ieee80211_sta::twt::IndividualTwtAgreement,
    ) -> Result<(), StationIndividualTwtHardwareError> {
        self.twt_install_count += 1;
        Ok(())
    }

    fn start_station_tbtt(&mut self, schedule: oer_esp32s31_hal::types::StaTbttSchedule) {
        self.tbtt = Some(schedule);
    }

    fn set_station_tbtt_interval(
        &mut self,
        interval_micros: u32,
    ) -> Result<(), oer_esp32s31_hal::types::StaTbttScheduleError> {
        let schedule = self
            .tbtt
            .as_mut()
            .ok_or(oer_esp32s31_hal::types::StaTbttScheduleError::NotScheduled)?;
        schedule.interval_micros = interval_micros;
        Ok(())
    }

    fn set_station_tbtt_ahead(&mut self, ahead_micros: u16, wake_ahead_micros: u16) {
        if let Some(schedule) = self.tbtt.as_mut() {
            schedule.ahead_micros = ahead_micros;
            schedule.wake_ahead_micros = wake_ahead_micros;
        }
    }

    fn stop_station_tbtt(&mut self) {
        self.tbtt = None;
    }

    fn set_power_save_tx_block(&mut self, blocked: bool) {
        self.tx_blocked = blocked;
    }

    fn set_rx_beacon_pti(&mut self, pti: oer_esp32s31_hal::types::MacPti) {
        self.beacon_pti = Some(pti);
    }

    fn clear_rx_beacon_pti(&mut self) {
        self.beacon_pti = None;
    }

    fn set_rx_beacon_time(&mut self, _window_micros: u16, _time_micros: u32) {}

    fn replace_sta_group_ccmp(
        &mut self,
        slot: &mut StaGroupCcmpSlot,
        current: &StaGroupCcmpKeyMaterial,
        replacement: &StaGroupCcmpKeyMaterial,
    ) -> Result<(), StaGroupCcmpReplaceError> {
        replace_sta_group_ccmp_with_rollback(self, slot, current, replacement)
    }
}

impl oer_esp32s31_ieee80211_mac::init::MacRuntimeStopHardware for Hardware {
    fn request_mac_runtime_stop(&mut self) {}

    fn mac_runtime_active_state(&mut self) -> u8 {
        0
    }

    fn resume_mac_runtime(&mut self) {}
}

fn make_connected_security(
    hardware: &mut Hardware,
) -> (ConnectedWpa2Security, StaCcmpRxReplayRxEndpoint, Ptk) {
    let (supplicant, ptk) = established_supplicant();
    let group = install_sta_group_ccmp(hardware, 1, &INITIAL_GTK).unwrap();
    let group_material = StaGroupCcmpKeyMaterial::new(1, INITIAL_GTK).unwrap();
    let replay =
        StaCcmpRxReplayEpoch::new([7, 0, 0, 0, 0, 0, 0, 0], 1, [5, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
    let (rx, control) = resource.start(replay).unwrap();
    (
        ConnectedWpa2Security::new(supplicant, group, group_material, control, None),
        rx,
        ptk,
    )
}

struct Power;

impl WifiTxPowerProfile for Power {
    fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
        WifiTxPowerPair {
            primary: 5,
            alternate: 6,
        }
    }
}

type Timer = oer_time_virtual::SkipClock;

fn completion(status: u8) -> MacTxCompletionObservation {
    MacTxCompletionObservation::new_model(status, 0)
}

fn make_tx<'a>(
    slot: Pin<&'a mut TxSlot<512>>,
    hardware: &mut Hardware,
) -> SingleMpduTx<'a, Power, fn() -> u32, Timer, 512> {
    fn entropy() -> u32 {
        0x1234_5678
    }

    let key = install_sta_pairwise_ccmp(hardware, BSSID, &[0x5a; 16]).unwrap();
    SingleMpduTx::new(
        oer_esp32s31_ieee80211_sta::single_mpdu_tx::WifiTxResources {
            slot,
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy,
            timer: Timer::default(),
        },
        oer_esp32s31_ieee80211_sta::single_mpdu_tx::ConnectedTxHandoff {
            security: oer_esp32s31_ieee80211_sta::single_mpdu_tx::ConnectedTxSecurity::Wpa2Personal(
                key,
            ),
            sequences: StaTxSequenceCounters::new(SequenceNumber::new(7).unwrap()),
            config: SingleMpduTxConfig {
                station_address: STATION,
                bssid: BSSID,
                ssid: oer_ieee80211_mac::ssid::WifiSsid::new(b"test").unwrap(),
                peer_qos: true,
                management_protection: false,
                access_category: WmmAccessCategory::BestEffort,
                control_schedule:
                    oer_espressif_ieee80211_policy::rate_control::DEFAULT_CONTROL_SCHEDULE,
                publication_timeout: oer_time::Duration::from_micros(250_000),
            },
        },
    )
}

fn finish_tx(
    hardware: &mut Hardware,
    tx: &mut SingleMpduTx<'_, Power, fn() -> u32, Timer, 512>,
    status: u8,
) {
    hardware.completion = Some(completion(status));
    assert_eq!(
        tx.service(
            hardware,
            WifiTxWake::Interrupt {
                events: oer_esp32s31_ieee80211_mac::irq::EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
}

fn idle_beacon() -> StaBeaconObservation {
    StaBeaconObservation {
        timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant::from_micros(1_000_000),
        // The association-owned policy below deliberately differs.
        interval_tu: 500,
        capability_information: 0,
        tim: Some(StaTimObservation {
            dtim_count: 1,
            dtim_period: 3,
            unicast_buffered: false,
            group_buffered: false,
        }),
        protection: StaBeaconProtection::UNPROTECTED,
        channel_switch: None,
    }
}

fn beacon_event(observation: StaBeaconObservation) -> ConnectedRxEvent<'static> {
    ConnectedRxEvent::Beacon {
        observation,
        metadata: MacRxMetadata::unavailable(),
        stamp: None,
    }
}

/// Wi-Fi and Bluetooth share the air: six periods of 100 TU, Wi-Fi owning
/// half of phase 0.
fn shared_coex() -> PowerCoexSnapshot {
    PowerCoexSnapshot {
        view: CoexView {
            active: true,
            current_period: 6,
            flexible_period: 1,
            interval: 1_024,
            phase0_share_percent: 50,
        },
        beacon_pti: oer_esp32s31_hal::types::MacPti::new(7).unwrap(),
    }
}

/// Wi-Fi shares the air with Bluetooth for the whole test.
struct SharedCoex;

impl crate::roles::station::power::PowerCoexSource for SharedCoex {
    fn power_coex(&self) -> PowerCoexSnapshot {
        shared_coex()
    }
}

fn join_beacon() -> JoinBeacon {
    JoinBeacon {
        beacon: PmBeacon {
            timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant::from_micros(1_000_000),
            interval_tu: 100,
            tim: None,
        },
        received_at: Some(oer_time::Instant::from_micros(0)),
    }
}

const WIFI_SLICE: CoexPhaseView = CoexPhaseView {
    share_percent: 50,
    wifi: CoexPhaseView::WIFI_SLICE | CoexPhaseView::SLICE_END,
};

fn individual_twt_parameters(implicit: bool) -> IndividualTwtParameterSet {
    IndividualTwtParameterSet {
        requesting_sta: true,
        setup_command: IndividualTwtSetupCommand::Request,
        trigger: false,
        implicit,
        flow_type: IndividualTwtFlowType::Announced,
        flow_id: IndividualTwtFlowId::new(2).unwrap(),
        wake_interval_exponent: 0,
        protection: false,
        target_wake_time_tsf: 10_000,
        nominal_minimum_wake_duration: 1,
        wake_interval_mantissa: 1_024,
        twt_channel: 0,
    }
}

#[test]
fn connected_runtime_binds_beacon_frontier_but_retains_software_monitor() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (_publisher, receiver) = resources.split();
    let policy =
        StaBeaconLossConfig::new(100, 10, oer_time::Duration::from_micros(1_024_000)).unwrap();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    control.enable_beacon_loss(policy);
    control
        .enable_hardware_beacon_monitor_frontier(
            StationBeaconMonitorBinding::new(
                BSSID,
                oer_ieee80211_mac::station_power_save::StaAssociationId::new(7).unwrap(),
            ),
            policy,
        )
        .unwrap();

    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        station_policy: Some(MacStaReceivePolicySnapshot {
            bssid: BSSID,
            association_id: 7,
            minimum_mpdu_start_spacing: 0,
            bssid_address_check_enabled: true,
            interface_is_soft_ap: false,
            interface_rx_policy_enabled: true,
            beacon_filter_control: 0,
        }),
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);

    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::Idle)
    );
    let frontier = control.hardware_beacon_monitor_frontier().unwrap();
    assert_eq!(
        frontier.reached(),
        oer_esp32s31_ieee80211_sta::hardware::beacon_monitor::StationHardwareBeaconMonitorStage::BeaconMissLimitRepresentable
    );
    assert_eq!(
        frontier.blocker(),
        oer_esp32s31_ieee80211_sta::hardware::beacon_monitor::StationHardwareBeaconMonitorBlocker::MissingBeaconMissTimeoutUnitConversion
    );
    assert!(!frontier.automatic_monitor_active());
    assert!(control.beacon_monitor().is_some());
}

#[test]
fn peer_accepted_explicit_twt_kicks_teardown_into_connected_tx() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    control.enable_individual_twt_requester(
        IndividualTwtRequesterConfig::new(
            oer_time::Duration::from_micros(1_000),
            oer_time::Duration::from_micros(100),
            1,
            1,
        )
        .unwrap(),
    );
    control
        .queue_individual_twt_setup(
            IndividualTwtProposal {
                control: IndividualTwtControl::REQUEST,
                parameters: individual_twt_parameters(true),
            },
            oer_time::Instant::from_micros(0),
        )
        .unwrap();

    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        twt_admit: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );

    let mut accepted = individual_twt_parameters(false);
    accepted.requesting_sta = false;
    accepted.setup_command = IndividualTwtSetupCommand::Accept;
    publisher.publish(ConnectedRxEvent::IndividualTwt {
        action: IndividualTwtAction::Setup(IndividualTwtSetup {
            dialog_token: 1,
            control: IndividualTwtControl::REQUEST,
            parameters: accepted,
        }),
        body: &[],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    assert_eq!(hardware.twt_install_count, 0);

    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::TxPending),
        "the explicit-information frontier must immediately reach teardown TX"
    );
    assert_eq!(
        control
            .individual_twt_requester()
            .unwrap()
            .status(IndividualTwtFlowId::new(2).unwrap()),
        oer_ieee80211_sta::twt::IndividualTwtFlowStatus::TeardownTransmitting
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    assert_eq!(
        control.individual_twt_runtime_evidence().actions_published,
        2
    );
}

const WPA2_RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];
const WPA2_SNONCE: [u8; 32] = [3; 32];
const WPA2_ANONCE: [u8; 32] = [4; 32];

fn owned_station_eapol(frame: &RsnTxFrame<512>) -> OwnedEapolFrame {
    OwnedEapolFrame::try_copy(RsnInterface::Station, BSSID, frame.as_bytes()).unwrap()
}

struct CompletedWpa2Fixture {
    security: ConnectedWpa2Security,
    replay_rx: StaCcmpRxReplayRxEndpoint,
    duplicate_message3: OwnedEapolFrame,
    bad_mic_message3: OwnedEapolFrame,
    wrong_replay_message3: OwnedEapolFrame,
}

fn completed_wpa2_fixture(hardware: &mut Hardware) -> CompletedWpa2Fixture {
    let pmk = Pmk::derive(b"password", b"ssid").unwrap();
    let ptk_context = PtkContext {
        authenticator_address: BSSID,
        supplicant_address: STATION,
        authenticator_nonce: WPA2_ANONCE,
        supplicant_nonce: WPA2_SNONCE,
    };
    let ptk = pmk.derive_ptk(oer_ieee80211_mac::security::rsn::Akm::Psk, ptk_context);
    let mut supplicant =
        RsnStaSupplicant::try_new(STATION, BSSID, WPA2_SNONCE, &WPA2_RSN, &WPA2_RSN, &[]).unwrap();
    let mut aes = RsnSoftwareAes::new();
    let message1 = RsnTxFrame::<512>::message1(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        STATION,
        1,
        WPA2_ANONCE,
    )
    .unwrap();
    let RsnStaSupplicantAction::Transmit(_) = embassy_futures::block_on(process_frame(
        &mut supplicant,
        owned_station_eapol(&message1),
        &pmk,
        &mut aes,
    ))
    .unwrap() else {
        panic!("Message 1 must produce Message 2")
    };
    let rsn = OwnedRsnIe::<64>::try_copy(&WPA2_RSN).unwrap();
    let gtk = RsnGtk::new(1, false, [0x6a; 16]).unwrap();
    let plain = RsnPlainKeyData::<64>::build(rsn.as_bytes(), &gtk, None).unwrap();
    let wrapped = software_aes128_key_wrap(ptk.kek(), plain.as_bytes()).unwrap();
    let message3 = RsnTxFrame::<512>::message3(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        STATION,
        2,
        WPA2_ANONCE,
        [0; 8],
        wrapped.as_bytes(),
    )
    .unwrap()
    .authenticate(&ptk);
    let RsnStaSupplicantAction::InstallKeys(request) = embassy_futures::block_on(process_frame(
        &mut supplicant,
        owned_station_eapol(&message3),
        &pmk,
        &mut aes,
    ))
    .unwrap() else {
        panic!("Message 3 must produce the initial key transaction")
    };
    let RsnStaSupplicantAction::Transmit(_) = supplicant
        .complete_key_install::<512>(request, true)
        .unwrap()
    else {
        panic!("successful key publication must produce Message 4")
    };

    let wrong_replay = RsnTxFrame::<512>::message3(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        STATION,
        3,
        WPA2_ANONCE,
        [0; 8],
        wrapped.as_bytes(),
    )
    .unwrap()
    .authenticate(&ptk);
    let mut forged = [0; 512];
    let length = message3.as_bytes().len();
    forged[..length].copy_from_slice(message3.as_bytes());
    forged[81] ^= 1;
    let bad_mic_message3 =
        OwnedEapolFrame::try_copy(RsnInterface::Station, BSSID, &forged[..length]).unwrap();
    let connected = supplicant.into_connected().unwrap();
    let group = install_sta_group_ccmp(hardware, 1, &[0x6a; 16]).unwrap();
    let group_material = StaGroupCcmpKeyMaterial::new(1, [0x6a; 16]).unwrap();
    let replay = StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap();
    let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
    let (replay_rx, replay_control) = resource.start(replay).unwrap();
    CompletedWpa2Fixture {
        security: ConnectedWpa2Security::new(
            connected,
            group,
            group_material,
            replay_control,
            None,
        ),
        replay_rx,
        duplicate_message3: owned_station_eapol(&message3),
        bad_mic_message3,
        wrong_replay_message3: owned_station_eapol(&wrong_replay),
    }
}

#[test]
fn duplicate_message3_reuses_connected_key_and_pn_while_forged_frames_are_ignored() {
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let CompletedWpa2Fixture {
        mut security,
        mut replay_rx,
        duplicate_message3,
        bad_mic_message3,
        wrong_replay_message3,
    } = completed_wpa2_fixture(&mut hardware);
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    assert_eq!(hardware.key_install_count, 2);

    let before = tx
        .take_protected_metadata(0)
        .unwrap()
        .expect("WPA2 TX owns pairwise metadata");
    assert_eq!(before.ccmp_header, [3, 0, 0, 0x20, 0, 0, 0, 0]);
    assert_eq!(
        embassy_futures::block_on(security.process(
            &mut hardware,
            &mut tx,
            ConnectedSecurityFrame::Unprotected(duplicate_message3),
        )),
        DatapathControlProgress::TxPending
    );
    assert_eq!(hardware.key_install_count, 2);
    assert_eq!(security.evidence().duplicate_message3, 1);
    assert_eq!(security.evidence().retransmitted, 1);
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(security.complete_tx(&mut tx), DatapathControlProgress::More);

    assert_eq!(
        embassy_futures::block_on(security.process(
            &mut hardware,
            &mut tx,
            ConnectedSecurityFrame::Unprotected(bad_mic_message3),
        )),
        DatapathControlProgress::More
    );
    assert_eq!(
        embassy_futures::block_on(security.process(
            &mut hardware,
            &mut tx,
            ConnectedSecurityFrame::Unprotected(wrong_replay_message3),
        )),
        DatapathControlProgress::More
    );
    let evidence = security.evidence();
    assert_eq!(evidence.duplicate_message3, 3);
    assert_eq!(evidence.ignored_duplicate_message3, 2);
    assert_eq!(evidence.retransmitted, 1);
    assert!(!evidence.tx_in_flight);
    assert_eq!(evidence.last_failure, None);
    assert_eq!(hardware.key_install_count, 2);

    let (_resources, handoff) = match tx.try_into_parts() {
        Ok(parts) => parts,
        Err(_) => panic!("completed TX must be idle"),
    };
    let oer_esp32s31_ieee80211_sta::single_mpdu_tx::ConnectedTxSecurity::Wpa2Personal(mut pairwise) =
        handoff.security
    else {
        panic!("connected TX must retain its installed pairwise key")
    };
    assert_eq!(
        pairwise.next_tx_ccmp_header().unwrap(),
        [9, 0, 0, 0x20, 0, 0, 0, 0]
    );
    let (_supplicant, group) = security.into_parts();
    replay_rx.stop().unwrap();
    group.clear(&mut hardware);
    pairwise.clear(&mut hardware);
}

#[test]
fn initial_tx_block_ack_requests_follow_zero_seven_five_and_arm_alarms() {
    let resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
    let (_publisher, receiver) = resources.split();
    let mut control = ConnectedControl::new(receiver, BSSID, true, tx_block_ack(), 1);
    control.queue_initial_tx_block_ack(2);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);

    for tid in STA_TX_BLOCK_ACK_TIDS {
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(DatapathControlProgress::TxPending)
        );
        finish_tx(&mut hardware, &mut tx, 0);
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(DatapathControlProgress::More)
        );
        assert!(control.tx_block_ack().alarm(tid).is_some());
    }
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::Idle)
    );

    embassy_futures::block_on(control.wait_ready(&mut tx));
    for tid in STA_TX_BLOCK_ACK_TIDS {
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(DatapathControlProgress::More)
        );
        assert_eq!(control.last_expired_tid(), Some(tid));
    }
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::TxPending),
        "a missing ADDBA response consumes one bounded retry"
    );
    assert!(control.tx_block_ack().alarm(0).is_some());
    finish_tx(&mut hardware, &mut tx, 0);
}

#[test]
fn rx_addba_hardware_is_committed_only_after_response_tx_success() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let reorder_resources = RxReorderCommandResources::<NoopRawMutex>::new();
    let (reorder_sender, reorder_receiver) = reorder_resources.split();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    control.set_rx_reorder_commands(reorder_sender);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    let action = BlockAckAction::AddbaRequest {
        dialog_token: 9,
        tid: 3,
        immediate: true,
        amsdu: false,
        window: 16,
        timeout_tu: 0,
        starting_sequence: SequenceNumber::new(0x123).unwrap(),
    };
    publisher.publish(ConnectedRxEvent::BlockAck {
        action,
        body: &[0; 9],
    });

    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::TxPending)
    );
    let agreement = hardware.programmed.unwrap();
    let snapshot = RxBlockAckSnapshot {
        hardware_index: agreement.hardware_index,
        interface: oer_esp32s31_ieee80211_mac::MacInterface::Station,
        peer: BSSID,
        tid: 3,
        starting_sequence: SequenceNumber::new(0x123).unwrap(),
        window: 16,
    };
    assert_eq!(agreement.tid, 3);
    assert!(
        control
            .rx_block_ack()
            .snapshots()
            .iter()
            .all(Option::is_none)
    );
    assert_eq!(
        try_receive_rx_reorder_command(&reorder_receiver),
        Some(RxReorderCommand::Start(snapshot))
    );

    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    assert_eq!(
        control.rx_block_ack().snapshots()[usize::from(agreement.hardware_index)]
            .unwrap()
            .tid,
        3
    );
    assert_eq!(hardware.clear_count, 0);

    publisher.publish(ConnectedRxEvent::BlockAck {
        action: BlockAckAction::Delba {
            tid: 3,
            initiator: true,
            reason: 37,
        },
        body: &[0; 6],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    assert_eq!(
        try_receive_rx_reorder_command(&reorder_receiver),
        Some(RxReorderCommand::Stop(snapshot.identity()))
    );
    assert_eq!(hardware.cleared[0], Some(agreement.hardware_index));
}

#[test]
fn failed_rx_addba_response_rolls_back_hardware_and_software() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let reorder_resources = RxReorderCommandResources::<NoopRawMutex>::new();
    let (reorder_sender, reorder_receiver) = reorder_resources.split();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    control.set_rx_reorder_commands(reorder_sender);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    let action = BlockAckAction::AddbaRequest {
        dialog_token: 9,
        tid: 3,
        immediate: true,
        amsdu: false,
        window: 16,
        timeout_tu: 0,
        starting_sequence: SequenceNumber::new(0x123).unwrap(),
    };
    publisher.publish(ConnectedRxEvent::BlockAck {
        action,
        body: &[0; 9],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::TxPending)
    );
    let agreement = hardware.programmed.unwrap();
    let snapshot = RxBlockAckSnapshot {
        hardware_index: agreement.hardware_index,
        interface: oer_esp32s31_ieee80211_mac::MacInterface::Station,
        peer: BSSID,
        tid: 3,
        starting_sequence: SequenceNumber::new(0x123).unwrap(),
        window: 16,
    };
    let hardware_index = agreement.hardware_index;
    assert!(matches!(
        try_receive_rx_reorder_command(&reorder_receiver),
        Some(RxReorderCommand::Start(observed)) if observed == snapshot
    ));

    // Status 2 is the vendor CTS-timeout retry path. Use the terminal
    // status-1 RTS error to exercise rollback after a failed response.
    finish_tx(&mut hardware, &mut tx, 1);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    assert_eq!(hardware.cleared[0], Some(hardware_index));
    assert_eq!(
        try_receive_rx_reorder_command(&reorder_receiver),
        Some(RxReorderCommand::Stop(snapshot.identity()))
    );
    assert!(
        control
            .rx_block_ack()
            .snapshots()
            .iter()
            .all(Option::is_none)
    );
    assert!(matches!(
        control.last_tx_failure(),
        Some(ConnectedControlTxFailure {
            kind: ConnectedControlTxKind::RxAddbaResponse { tid: 3 },
            outcome: SingleMpduTxOutcome::HardwareFailure(report),
        })
        if report
            .completion
            .is_some_and(|completion| completion.status() == 1)
    ));
}

#[test]
fn tx_addba_response_and_delba_toggle_he_tid_ownership() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = ConnectedControl::new(receiver, BSSID, true, tx_block_ack(), 1);
    control.queue_initial_tx_block_ack(1);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    publisher.publish(ConnectedRxEvent::BlockAck {
        action: BlockAckAction::AddbaResponse {
            dialog_token: 42,
            status: 0,
            tid: 0,
            immediate: true,
            amsdu: true,
            window: 16,
            timeout_tu: 0,
        },
        body: &[0; 9],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    assert_eq!(control.stale_tx_block_ack_responses(), 1);
    assert_eq!(control.last_stale_tx_block_ack_token(), Some(42));
    assert!(control.tx_block_ack().alarm(0).is_some());

    publisher.publish(ConnectedRxEvent::BlockAck {
        action: BlockAckAction::AddbaResponse {
            dialog_token: 1,
            status: 0,
            tid: 0,
            immediate: true,
            amsdu: true,
            window: 16,
            timeout_tu: 0,
        },
        body: &[0; 9],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    assert_eq!(hardware.he_tid[0], Some((0, true)));

    publisher.publish(ConnectedRxEvent::BlockAck {
        action: BlockAckAction::Delba {
            tid: 0,
            initiator: false,
            reason: 37,
        },
        body: &[0; 6],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    assert_eq!(hardware.he_tid[1], Some((0, false)));
}

#[test]
fn beacon_loss_disconnects_only_after_bounded_active_probes() {
    let resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
    let (_publisher, receiver) = resources.split();
    let mut control = ConnectedControl::new(receiver, BSSID, true, tx_block_ack(), 1);
    control.enable_beacon_loss(
        StaBeaconLossConfig::new(100, 3, oer_time::Duration::from_micros(307_200)).unwrap(),
    );
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);

    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::Idle)
    );
    for _ in 0..5 {
        embassy_futures::block_on(control.wait_ready(&mut tx));
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(DatapathControlProgress::TxPending)
        );
        finish_tx(&mut hardware, &mut tx, 0);
        assert_eq!(
            embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
            Ok(DatapathControlProgress::More)
        );
    }
    embassy_futures::block_on(control.wait_ready(&mut tx));
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::Exit(
            oer_esp32s31_ieee80211_sta::connected_control::ConnectedDisconnectReason::BeaconLoss
        ))
    );
    assert!(control.beacon_lost());
    assert_eq!(
        hardware.he_tid[..3],
        [Some((0, false)), Some((7, false)), Some((5, false))]
    );
}

#[test]
fn associated_probe_response_cancels_beacon_loss_recovery() {
    let resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    control.enable_beacon_loss(
        StaBeaconLossConfig::new(100, 3, oer_time::Duration::from_micros(307_200)).unwrap(),
    );
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);

    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::Idle)
    );
    embassy_futures::block_on(control.wait_ready(&mut tx));
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );

    publisher.publish(ConnectedRxEvent::ProbeResponse);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    assert!(!control.beacon_lost());
    assert_eq!(
        control.beacon_monitor().unwrap().deadline(),
        Some(oer_time::Instant::from_micros(614_400))
    );
}

#[test]
fn peer_deauthentication_disconnects_with_its_reason_code() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);

    publisher.publish(ConnectedRxEvent::PeerDisconnect(StaDisconnect {
        kind: StaDisconnectKind::Deauthentication,
        reason_code: 4,
    }));
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::Exit(
            oer_esp32s31_ieee80211_sta::connected_control::ConnectedDisconnectReason::PeerDeauthentication {
                reason_code: 4,
            }
        ))
    );
}

#[test]
fn mailbox_overflow_fails_closed_before_processing_an_incomplete_event_stream() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    let action = BlockAckAction::Delba {
        tid: 1,
        initiator: true,
        reason: 37,
    };

    publisher.publish(ConnectedRxEvent::BlockAck {
        action,
        body: &[3, 2, 0, 0, 2, 0],
    });
    publisher.publish(ConnectedRxEvent::BlockAck {
        action,
        body: &[3, 2, 0, 0, 2, 0],
    });

    assert!(control.has_immediate_work());
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::Exit(
            oer_esp32s31_ieee80211_sta::connected_control::ConnectedDisconnectReason::ControlMailboxOverflow,
        ))
    );
}

#[test]
fn shutdown_clears_rx_tx_block_ack_and_discards_late_control_events() {
    let resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = ConnectedControl::new(receiver, BSSID, true, tx_block_ack(), 1);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);

    publisher.publish(ConnectedRxEvent::BlockAck {
        action: BlockAckAction::AddbaRequest {
            dialog_token: 9,
            tid: 3,
            immediate: true,
            amsdu: false,
            window: 16,
            timeout_tu: 0,
            starting_sequence: SequenceNumber::new(0x123).unwrap(),
        },
        body: &[0; 9],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );

    control.queue_initial_tx_block_ack(1);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::TxPending)
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    publisher.publish(ConnectedRxEvent::BlockAck {
        action: BlockAckAction::AddbaResponse {
            dialog_token: 1,
            status: 0,
            tid: 0,
            immediate: true,
            amsdu: true,
            window: 16,
            timeout_tu: 0,
        },
        body: &[0; 9],
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    publisher.publish(beacon_event(idle_beacon()));

    assert_eq!(
        control.shutdown(&mut hardware, &mut tx),
        Ok(ConnectedControlShutdown {
            rx_block_ack_agreements: 1,
            tx_block_ack_sessions: 1,
            discarded_events: 1,
            in_flight: None,
            hardware_beacon_monitor: None,
        })
    );
    assert_eq!(hardware.cleared[0], Some(0));
    assert_eq!(
        hardware.he_tid,
        [
            Some((0, true)),
            Some((0, false)),
            Some((7, false)),
            Some((5, false)),
        ]
    );
    assert!(
        control
            .rx_block_ack()
            .snapshots()
            .into_iter()
            .all(|agreement| agreement.is_none())
    );
    assert!(
        STA_TX_BLOCK_ACK_TIDS.into_iter().all(|tid| control
            .tx_block_ack()
            .operational(tid)
            .is_none()
            && control.tx_block_ack().alarm(tid).is_none())
    );
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::Idle)
    );
}

#[test]
fn station_shutdown_preserves_access_point_rx_block_ack_banks() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (_publisher, receiver) = resources.split();
    let mut control = ConnectedControl::new(receiver, BSSID, true, tx_block_ack(), 1);
    control
        .rx_block_ack()
        .offer(RxBlockAckRequest {
            interface: MacInterface::AccessPoint,
            peer: [0x30, 0x31, 0x32, 0x33, 0x34, 0x35],
            dialog_token: 1,
            tid: 5,
            immediate: true,
            requested_window: 16,
            timeout_tu: 0,
            starting_sequence: SequenceNumber::new(7).unwrap(),
        })
        .unwrap();
    let activation = control.rx_block_ack().begin_pending().unwrap().unwrap();
    let ap_agreement = activation.negotiated();
    control.rx_block_ack().commit(activation).unwrap();

    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware::default();
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    assert_eq!(
        control.shutdown(&mut hardware, &mut tx),
        Ok(ConnectedControlShutdown::default())
    );
    assert_eq!(hardware.clear_count, 0);
    assert_eq!(
        control
            .rx_block_ack()
            .snapshots_for(MacInterface::AccessPoint)[usize::from(ap_agreement.hardware_index)],
        Some(ap_agreement)
    );
}

#[test]
fn beacon_received_on_exact_deadline_refreshes_before_loss_check() {
    let resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    control.enable_beacon_loss(
        StaBeaconLossConfig::new(100, 3, oer_time::Duration::from_micros(307_200)).unwrap(),
    );
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);

    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::Idle)
    );
    embassy_futures::block_on(control.wait_ready(&mut tx));
    publisher.publish(beacon_event(StaBeaconObservation {
        timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant::from_micros(123),
        interval_tu: 100,
        capability_information: 0,
        tim: None,
        protection: StaBeaconProtection::UNPROTECTED,
        channel_switch: None,
    }));
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    assert!(!control.beacon_lost());
    assert_eq!(
        control.beacon_monitor().unwrap().deadline(),
        Some(oer_time::Instant::from_micros(614_400))
    );
}

#[test]
fn connected_beacon_protection_updates_the_tx_bss_facts() {
    use oer_esp32s31_ieee80211_mac::tx::protection::{
        BasicRates, BssProtection, HePacketPadding, HeTxopDurationRtsThreshold,
    };
    use oer_ieee80211_mac::protection::{ErpProtection, HtProtectionMode};

    let resources = ConnectedControlResources::<NoopRawMutex, 8>::new();
    let (mut publisher, receiver) = resources.split();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    let associated = BssProtection {
        basic_rates: BasicRates::from_rate_elements(&[0x82, 0x84], &[]),
        short_preamble: true,
        he_packet_padding: HePacketPadding::Us16,
        ..BssProtection::UNPROTECTED
    };
    tx.policy_mut().install_bss_protection(associated);

    publisher.publish(beacon_event(StaBeaconObservation {
        timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant::from_micros(123),
        interval_tu: 100,
        capability_information: 0,
        tim: None,
        protection: StaBeaconProtection {
            erp: ErpProtection::new(true, false),
            ht: HtProtectionMode::NonHtMixed,
            he_txop_rts_threshold: Some(64),
        },
        channel_switch: None,
    }));
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    assert_eq!(
        tx.policy().protection().bss(),
        BssProtection {
            erp: ErpProtection::new(true, false),
            ht: HtProtectionMode::NonHtMixed,
            he_txop_rts_threshold: HeTxopDurationRtsThreshold::new(64),
            ..associated
        }
    );

    // The BSS later drops every requirement.
    publisher.publish(beacon_event(StaBeaconObservation {
        timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant::from_micros(204_923),
        interval_tu: 100,
        capability_information: 0,
        tim: None,
        protection: StaBeaconProtection::UNPROTECTED,
        channel_switch: None,
    }));
    embassy_futures::block_on(control.service(&mut hardware, &mut tx)).unwrap();
    assert_eq!(tx.policy().protection().bss(), associated);
}

fn service(
    control: &mut ConnectedControl<'_, NoopRawMutex, 4>,
    hardware: &mut Hardware,
    tx: &mut SingleMpduTx<'_, Power, fn() -> u32, Timer, 512>,
    context: DatapathControlContext,
) -> DatapathControlProgress<ConnectedDisconnectReason> {
    embassy_futures::block_on(control.service_with_context(hardware, tx, context)).unwrap()
}

/// Run control until it waits, the agent performing each command.
fn settle(
    control: &mut ConnectedControl<'_, NoopRawMutex, 4>,
    link: &StationPowerLink<NoopRawMutex>,
    hardware: &mut Hardware,
    tx: &mut SingleMpduTx<'_, Power, fn() -> u32, Timer, 512>,
    context: DatapathControlContext,
    performed: &mut std::vec::Vec<ConnectedPowerCommand>,
) -> DatapathControlProgress<ConnectedDisconnectReason> {
    loop {
        let progress = service(control, hardware, tx, context);
        link.perform_for_test(&mut |command| performed.push(command), None, false);
        if progress != DatapathControlProgress::More {
            return progress;
        }
    }
}

#[test]
fn a_shared_station_leaves_the_air_at_its_slice_end_and_holds_its_frames() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (_publisher, receiver) = resources.split();
    let link = StationPowerLink::<NoopRawMutex>::new();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    control.enable_power_management(SleepType::None, join_beacon(), link.bind(&SharedCoex));
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    let mut performed = std::vec::Vec::new();

    // The start places the TBTT on the join beacon's boundary and publishes
    // the beacon priority and the schedule interval.
    settle(
        &mut control,
        &link,
        &mut hardware,
        &mut tx,
        DatapathControlContext::IDLE,
        &mut performed,
    );
    // Nine beacon intervals of 102.4 ms precede the join timestamp.
    assert_eq!(hardware.tbtt.unwrap().first_tbtt_tsf, 9 * 102_400);
    assert_eq!(hardware.beacon_pti, oer_esp32s31_hal::types::MacPti::new(7));
    assert!(performed.contains(&ConnectedPowerCommand::Coex(
        PmCoexAction::SetFlexiblePeriod(1)
    )));

    // The Wi-Fi phase requests the slice and arms its end.
    link.perform_for_test(&mut |_| {}, Some(WIFI_SLICE), false);
    settle(
        &mut control,
        &link,
        &mut hardware,
        &mut tx,
        DatapathControlContext::IDLE,
        &mut performed,
    );
    assert!(performed.iter().any(|command| matches!(
        command,
        ConnectedPowerCommand::Coex(PmCoexAction::Request {
            event: PmCoexEvent::Slice,
            ..
        })
    )));
    assert!(control.admits_frames());

    // At the slice end the queues block and PM=1 goes out.
    let deadline = control.next_alarm_deadline().unwrap();
    embassy_futures::block_on(tx.wait_until(deadline));
    assert_eq!(
        settle(
            &mut control,
            &link,
            &mut hardware,
            &mut tx,
            DatapathControlContext::IDLE,
            &mut performed,
        ),
        DatapathControlProgress::TxPending
    );
    assert!(hardware.tx_blocked);
    assert!(!control.admits_frames());
    assert_eq!(control.power_management().state(), PmState::PowerSave);

    // Traffic queued outside the slice waits for the next one.
    finish_tx(&mut hardware, &mut tx, 0);
    let pending = DatapathControlContext {
        network_tx_pending: true,
        stop_pending: false,
    };
    settle(
        &mut control,
        &link,
        &mut hardware,
        &mut tx,
        pending,
        &mut performed,
    );
    assert!(!control.admits_frames());

    // The next TBTT restarts the phases at the Wi-Fi slice and wakes the
    // station for its frame, which carries PM=0.
    link.perform_for_test(&mut |_| {}, Some(WIFI_SLICE), true);
    assert_eq!(
        settle(
            &mut control,
            &link,
            &mut hardware,
            &mut tx,
            pending,
            &mut performed,
        ),
        DatapathControlProgress::TxPending
    );
    assert!(!control.admits_frames());
    finish_tx(&mut hardware, &mut tx, 0);
    settle(
        &mut control,
        &link,
        &mut hardware,
        &mut tx,
        pending,
        &mut performed,
    );
    assert!(!hardware.tx_blocked);
    assert_eq!(control.power_management().state(), PmState::Awake);
    assert!(control.admits_frames());
}

#[test]
fn frames_wait_until_the_agent_performed_the_commands_before_them() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (_publisher, receiver) = resources.split();
    let link = StationPowerLink::<NoopRawMutex>::new();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    control.enable_power_management(SleepType::None, join_beacon(), link.bind(&SharedCoex));
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    service(
        &mut control,
        &mut hardware,
        &mut tx,
        DatapathControlContext::IDLE,
    );
    assert!(link.outstanding());
    assert!(!control.admits_frames());
    link.perform_for_test(&mut |_| {}, None, false);
    assert!(control.admits_frames());
}

#[test]
fn a_controlled_stop_wakes_and_sends_one_leaving_deauthentication() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (_publisher, receiver) = resources.split();
    let link = StationPowerLink::<NoopRawMutex>::new();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    control.enable_power_management(SleepType::None, join_beacon(), link.bind(&SharedCoex));
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    let mut performed = std::vec::Vec::new();
    settle(
        &mut control,
        &link,
        &mut hardware,
        &mut tx,
        DatapathControlContext::IDLE,
        &mut performed,
    );
    assert!(control.leave_pending());

    assert_eq!(
        service(
            &mut control,
            &mut hardware,
            &mut tx,
            DatapathControlContext::STOPPING
        ),
        DatapathControlProgress::TxPending
    );
    // The station woke before its Deauthentication left.
    assert!(!control.power_management().is_started());
    assert!(!hardware.tx_blocked);
    // An unacknowledged Deauthentication is not retried.
    finish_tx(&mut hardware, &mut tx, 1);
    assert_eq!(
        service(
            &mut control,
            &mut hardware,
            &mut tx,
            DatapathControlContext::STOPPING
        ),
        DatapathControlProgress::More
    );
    assert!(!control.leave_pending());
    assert_eq!(
        service(
            &mut control,
            &mut hardware,
            &mut tx,
            DatapathControlContext::STOPPING
        ),
        DatapathControlProgress::Idle
    );
    control.shutdown(&mut hardware, &mut tx).unwrap();
}

#[test]
fn shutdown_stops_power_management_and_hands_the_releases_to_the_agent() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (_publisher, receiver) = resources.split();
    let link = StationPowerLink::<NoopRawMutex>::new();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    control.enable_power_management(SleepType::None, join_beacon(), link.bind(&SharedCoex));
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    let mut performed = std::vec::Vec::new();
    settle(
        &mut control,
        &link,
        &mut hardware,
        &mut tx,
        DatapathControlContext::IDLE,
        &mut performed,
    );
    assert!(hardware.tbtt.is_some());

    control.shutdown(&mut hardware, &mut tx).unwrap();
    assert!(hardware.tbtt.is_none());
    assert!(!hardware.tx_blocked);
    assert!(!control.power_management().is_started());
}

#[test]
fn group_rekey_replaces_different_key_id_before_message2() {
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let (mut security, mut replay_rx, ptk) = make_connected_security(&mut hardware);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    let installs_before = hardware.key_install_count;

    assert_eq!(
        embassy_futures::block_on(security.process(
            &mut hardware,
            &mut tx,
            group_message1(&ptk, 3, 2, [0x22; 16], [9, 0, 0, 0, 0, 0, 0, 0],),
        )),
        DatapathControlProgress::TxPending
    );
    assert_eq!(hardware.ccmp_clears, 1);
    assert_eq!(hardware.key_install_count, installs_before + 1);
    assert_eq!(
        security.evidence(),
        ConnectedWpa2SecurityEvidence {
            replay_counter: 3,
            group_message1: 1,
            duplicate_message3: 0,
            ignored_duplicate_message3: 0,
            last_ignored_duplicate_message3: None,
            installed: 1,
            retransmitted: 0,
            tx_in_flight: true,
            last_failure: None,
        }
    );

    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(security.complete_tx(&mut tx), DatapathControlProgress::More);
    let (_, group) = security.into_parts();
    assert_eq!(group.key_id(), 2);
    replay_rx.stop().unwrap();
    group.clear(&mut hardware);
}

#[test]
fn group_rekey_rejects_a_retired_key_id_before_hardware_mutation() {
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let (mut security, mut replay_rx, ptk) = make_connected_security(&mut hardware);
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut tx = make_tx(slot.as_mut(), &mut hardware);

    assert_eq!(
        embassy_futures::block_on(security.process(
            &mut hardware,
            &mut tx,
            group_message1(&ptk, 3, 2, [0x22; 16], [9, 0, 0, 0, 0, 0, 0, 0],),
        )),
        DatapathControlProgress::TxPending
    );
    finish_tx(&mut hardware, &mut tx, 0);
    assert_eq!(security.complete_tx(&mut tx), DatapathControlProgress::More);
    let clears_after_key_two = hardware.ccmp_clears;
    let installs_after_key_two = hardware.key_install_count;

    assert_eq!(
        embassy_futures::block_on(security.process(
            &mut hardware,
            &mut tx,
            group_message1(&ptk, 4, 1, [0x33; 16], [10, 0, 0, 0, 0, 0, 0, 0],),
        )),
        DatapathControlProgress::Exit(ConnectedDisconnectReason::GroupKeyHandshakeFailed)
    );
    assert_eq!(hardware.ccmp_clears, clears_after_key_two);
    assert_eq!(hardware.key_install_count, installs_after_key_two);
    assert_eq!(security.evidence().installed, 1);
    assert_eq!(security.evidence().replay_counter, 3);
    assert_eq!(
        security.evidence().last_failure,
        Some(ConnectedWpa2SecurityFailure::RetiredKeyIdGenerationUnavailable { key_id: 1 })
    );

    let (_, group) = security.into_parts();
    assert_eq!(group.key_id(), 2);
    replay_rx.stop().unwrap();
    group.clear(&mut hardware);
}

#[test]
fn same_key_id_rekey_is_rsc_only_for_identical_gtk_and_rejects_changed_gtk() {
    {
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let (mut security, mut replay_rx, ptk) = make_connected_security(&mut hardware);
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut tx = make_tx(slot.as_mut(), &mut hardware);
        let installs_before = hardware.key_install_count;

        assert_eq!(
            embassy_futures::block_on(security.process(
                &mut hardware,
                &mut tx,
                group_message1(&ptk, 3, 1, INITIAL_GTK, [9, 0, 0, 0, 0, 0, 0, 0],),
            )),
            DatapathControlProgress::TxPending
        );
        assert_eq!(hardware.ccmp_clears, 0);
        assert_eq!(hardware.key_install_count, installs_before);
        assert_eq!(security.evidence().installed, 1);
        assert_eq!(security.evidence().replay_counter, 3);

        let (_, group) = security.into_parts();
        assert_eq!(group.key_id(), 1);
        replay_rx.stop().unwrap();
        group.clear(&mut hardware);
    }

    {
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let (mut security, mut replay_rx, ptk) = make_connected_security(&mut hardware);
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut tx = make_tx(slot.as_mut(), &mut hardware);
        let installs_before = hardware.key_install_count;

        assert_eq!(
            embassy_futures::block_on(security.process(
                &mut hardware,
                &mut tx,
                group_message1(&ptk, 3, 1, [0x33; 16], [9, 0, 0, 0, 0, 0, 0, 0],),
            )),
            DatapathControlProgress::Exit(ConnectedDisconnectReason::GroupKeyHandshakeFailed)
        );
        assert_eq!(hardware.ccmp_clears, 0);
        assert_eq!(hardware.key_install_count, installs_before);
        assert_eq!(security.evidence().installed, 0);
        assert_eq!(security.evidence().replay_counter, 2);
        assert_eq!(
            security.evidence().last_failure,
            Some(ConnectedWpa2SecurityFailure::SameKeyIdGenerationUnavailable)
        );

        let (_, group) = security.into_parts();
        assert_eq!(group.key_id(), 1);
        replay_rx.stop().unwrap();
        group.clear(&mut hardware);
    }
}

#[test]
fn group_rekey_hardware_failure_restores_old_epoch_or_quarantines() {
    {
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let (mut security, mut replay_rx, ptk) = make_connected_security(&mut hardware);
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut tx = make_tx(slot.as_mut(), &mut hardware);
        hardware.ccmp_reject_installs = 1;

        assert_eq!(
            embassy_futures::block_on(security.process(
                &mut hardware,
                &mut tx,
                group_message1(&ptk, 3, 2, [0x22; 16], [9, 0, 0, 0, 0, 0, 0, 0],),
            )),
            DatapathControlProgress::Exit(ConnectedDisconnectReason::GroupKeyHandshakeFailed)
        );
        assert_eq!(hardware.ccmp_clears, 1);
        assert_eq!(
            security.evidence().last_failure,
            Some(ConnectedWpa2SecurityFailure::KeyReplace(
                StaGroupCcmpReplaceError::ReplacementRolledBack(CryptoKeyError::HardwareRejected)
            ))
        );
        assert_eq!(security.evidence().replay_counter, 2);
        assert_eq!(security.evidence().installed, 0);

        let (_, group) = security.into_parts();
        assert_eq!(group.key_id(), 1);
        replay_rx.stop().unwrap();
        group.clear(&mut hardware);
    }

    {
        let mut hardware = Hardware {
            prepare: true,
            ..Hardware::default()
        };
        let (mut security, mut replay_rx, ptk) = make_connected_security(&mut hardware);
        let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
        let mut tx = make_tx(slot.as_mut(), &mut hardware);
        hardware.ccmp_reject_installs = 2;

        assert_eq!(
            embassy_futures::block_on(security.process(
                &mut hardware,
                &mut tx,
                group_message1(&ptk, 3, 2, [0x22; 16], [9, 0, 0, 0, 0, 0, 0, 0],),
            )),
            DatapathControlProgress::Exit(ConnectedDisconnectReason::GroupKeyHandshakeFailed)
        );
        assert_eq!(hardware.ccmp_clears, 1);
        assert_eq!(
            security.evidence().last_failure,
            Some(ConnectedWpa2SecurityFailure::KeyReplace(
                StaGroupCcmpReplaceError::RollbackFailed {
                    replacement: CryptoKeyError::HardwareRejected,
                    rollback: CryptoKeyError::HardwareRejected,
                }
            ))
        );
        assert_eq!(security.evidence().replay_counter, 2);
        assert_eq!(security.evidence().installed, 0);

        let (_, group) = security.into_parts();
        assert_eq!(group.key_id(), 1);
        replay_rx.stop().unwrap();
        group.clear(&mut hardware);
    }
}

fn management_protected_control<'a>(
    resources: &'a ConnectedControlResources<NoopRawMutex, 4>,
    hardware: &mut Hardware,
) -> (
    crate::roles::station::control_mailbox::ConnectedControlPublisher<'a, NoopRawMutex, 4>,
    ConnectedControl<'a, NoopRawMutex, 4>,
) {
    fn transaction_seed() -> u32 {
        0x0102
    }

    let (publisher, receiver) = resources.split();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    let (supplicant, _ptk) = established_supplicant();
    let group = install_sta_group_ccmp(hardware, 1, &INITIAL_GTK).unwrap();
    let group_material = StaGroupCcmpKeyMaterial::new(1, INITIAL_GTK).unwrap();
    let replay = StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap();
    let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
    let (_rx, replay_control) = resource.start(replay).unwrap();
    let management = oer_esp32s31_ieee80211_sta::connected::management_protection::StationManagementProtection::new(
        [0x5a; 16],
        &oer_ieee80211_rsn::frames::RsnIgtk::new(4, [0; 6], [0x77; 16]).unwrap(),
    );
    assert!(
        control
            .install_wpa2_security(
                ConnectedWpa2Security::new(
                    supplicant,
                    group,
                    group_material,
                    replay_control,
                    Some(management),
                ),
                transaction_seed,
            )
            .is_ok()
    );
    (publisher, control)
}

#[test]
fn an_unanswered_sa_query_ends_the_association_after_1024_ms() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    let (mut publisher, mut control) = management_protected_control(&resources, &mut hardware);

    publisher.publish(ConnectedRxEvent::UnprotectedDisconnect(StaDisconnect {
        kind: StaDisconnectKind::Deauthentication,
        reason_code: 7,
    }));
    // The first Request, then one every 200 ms until the timeout.
    let mut requests = 0;
    loop {
        embassy_futures::block_on(control.wait_ready(&mut tx));
        match embassy_futures::block_on(control.service(&mut hardware, &mut tx)) {
            Ok(DatapathControlProgress::TxPending) => {
                requests += 1;
                finish_tx(&mut hardware, &mut tx, 0);
                assert_eq!(
                    embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
                    Ok(DatapathControlProgress::More)
                );
            }
            Ok(DatapathControlProgress::Exit(reason)) => {
                assert_eq!(
                    reason,
                    oer_esp32s31_ieee80211_sta::connected_control::ConnectedDisconnectReason::SaQueryTimeout
                );
                break;
            }
            other => panic!("unexpected control progress {other:?}"),
        }
    }
    assert_eq!(requests, 6);
    assert_eq!(tx.now().as_micros(), 1_024_000);
}

#[test]
fn a_forged_protected_frame_is_counted_and_keeps_the_association() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    let (mut publisher, mut control) = management_protected_control(&resources, &mut hardware);

    let mut forged = [0_u8; 50];
    forged[..2].copy_from_slice(&[0xc0, 0x40]);
    forged[4..10].copy_from_slice(&STATION);
    forged[10..16].copy_from_slice(&BSSID);
    forged[16..22].copy_from_slice(&BSSID);
    forged[27] = 0x20;
    publisher.publish(ConnectedRxEvent::ProtectedManagement {
        frame: &forged,
        group: false,
    });
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::More)
    );
    assert_eq!(control.management_protection_drops().unverified, 1);
    assert_eq!(
        embassy_futures::block_on(control.service(&mut hardware, &mut tx)),
        Ok(DatapathControlProgress::Idle)
    );
}

#[test]
fn the_station_takes_the_access_point_tsf_at_power_start_and_from_each_beacon() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let link = StationPowerLink::<NoopRawMutex>::new();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    // The join beacon arrived 5 ms before power management starts.
    control.enable_power_management(
        SleepType::None,
        JoinBeacon {
            received_at: Some(oer_time::Instant::from_micros(1_000)),
            ..join_beacon()
        },
        link.bind(&SharedCoex),
    );
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        station_tsf: 7,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    embassy_futures::block_on(tx.wait_until(oer_time::Instant::from_micros(6_000)));
    let mut performed = std::vec::Vec::new();
    settle(
        &mut control,
        &link,
        &mut hardware,
        &mut tx,
        DatapathControlContext::IDLE,
        &mut performed,
    );
    assert_eq!(hardware.station_tsf, 1_000_000 + 5_000);

    // A beacon stamped at MAC local time 10 000 and followed at 12 000
    // carries its own TSF, advanced by the counter's difference.
    hardware.mac_local_time = 12_000;
    publisher.publish(ConnectedRxEvent::Beacon {
        observation: StaBeaconObservation {
            timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant::from_micros(2_000_000),
            ..idle_beacon()
        },
        metadata: MacRxMetadata::unavailable(),
        stamp: Some(10_000),
    });
    settle(
        &mut control,
        &link,
        &mut hardware,
        &mut tx,
        DatapathControlContext::IDLE,
        &mut performed,
    );
    assert_eq!(hardware.station_tsf, 2_000_000 + 2_000);

    // The counter's difference carries across its 32-bit wrap.
    hardware.mac_local_time = 1_000;
    publisher.publish(ConnectedRxEvent::Beacon {
        observation: StaBeaconObservation {
            timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant::from_micros(3_000_000),
            ..idle_beacon()
        },
        metadata: MacRxMetadata::unavailable(),
        stamp: Some(u32::MAX - 999),
    });
    settle(
        &mut control,
        &link,
        &mut hardware,
        &mut tx,
        DatapathControlContext::IDLE,
        &mut performed,
    );
    assert_eq!(hardware.station_tsf, 3_000_000 + 2_000);

    // A beacon without a receive timestamp leaves the TSF as it is.
    publisher.publish(ConnectedRxEvent::Beacon {
        observation: StaBeaconObservation {
            timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant::from_micros(9_000_000),
            ..idle_beacon()
        },
        metadata: MacRxMetadata::unavailable(),
        stamp: None,
    });
    settle(
        &mut control,
        &link,
        &mut hardware,
        &mut tx,
        DatapathControlContext::IDLE,
        &mut performed,
    );
    assert_eq!(hardware.station_tsf, 3_000_000 + 2_000);
}

#[test]
fn a_join_tsf_advanced_past_its_range_ends_the_association_before_any_write() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (_publisher, receiver) = resources.split();
    let link = StationPowerLink::<NoopRawMutex>::new();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    // A join timestamp 4 µs before the end of the TSF, received 5 ms ago.
    control.enable_power_management(
        SleepType::None,
        JoinBeacon {
            beacon: PmBeacon {
                timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant::from_micros(u64::MAX - 4),
                ..join_beacon().beacon
            },
            received_at: Some(oer_time::Instant::from_micros(1_000)),
        },
        link.bind(&SharedCoex),
    );
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        station_tsf: 7,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    embassy_futures::block_on(tx.wait_until(oer_time::Instant::from_micros(6_000)));
    let ended = loop {
        match embassy_futures::block_on(control.service_with_context(
            &mut hardware,
            &mut tx,
            DatapathControlContext::IDLE,
        )) {
            Ok(DatapathControlProgress::More) => {}
            other => break other,
        }
    };
    // A disconnect, which refreshes the candidate, not a control failure,
    // which would stop the station.
    assert_eq!(
        ended,
        Ok(DatapathControlProgress::Exit(
            ConnectedDisconnectReason::JoinTsfUnrepresentable
        ))
    );
    assert_eq!((hardware.station_tsf, hardware.station_tsf_writes), (7, 0));
}

#[test]
fn a_beacon_queued_behind_power_inputs_takes_the_next_step() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let link = StationPowerLink::<NoopRawMutex>::new();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    control.enable_power_management(SleepType::None, join_beacon(), link.bind(&SharedCoex));
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    let mut performed = std::vec::Vec::new();
    settle(
        &mut control,
        &link,
        &mut hardware,
        &mut tx,
        DatapathControlContext::IDLE,
        &mut performed,
    );

    // Under saturated traffic the TBTT and the beacon after it wait for
    // the same control step, and a new power input waits at every step.
    embassy_futures::block_on(tx.wait_until(oer_time::Instant::from_micros(12_000)));
    hardware.mac_local_time = 12_000;
    publisher.publish(ConnectedRxEvent::Beacon {
        observation: StaBeaconObservation {
            timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant::from_micros(2_000_000),
            ..idle_beacon()
        },
        metadata: MacRxMetadata::unavailable(),
        stamp: Some(10_000),
    });
    link.perform_for_test(&mut |command| performed.push(command), None, true);
    // The TBTT goes first; the Null it starts completes before any event.
    assert_eq!(
        service(
            &mut control,
            &mut hardware,
            &mut tx,
            DatapathControlContext::IDLE
        ),
        DatapathControlProgress::TxPending
    );
    finish_tx(&mut hardware, &mut tx, 0);
    service(
        &mut control,
        &mut hardware,
        &mut tx,
        DatapathControlContext::IDLE,
    );
    assert_ne!(hardware.station_tsf, 2_000_000 + 2_000);
    link.perform_for_test(&mut |command| performed.push(command), None, true);
    service(
        &mut control,
        &mut hardware,
        &mut tx,
        DatapathControlContext::IDLE,
    );
    assert_eq!(hardware.station_tsf, 2_000_000 + 2_000);
}

#[test]
fn the_role_writes_the_station_tsf_through_its_owner_at_power_start_and_each_beacon() {
    let resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (mut publisher, receiver) = resources.split();
    let link = StationPowerLink::<NoopRawMutex>::new();
    let mut control = ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), 1);
    control.enable_power_management(
        SleepType::None,
        JoinBeacon {
            received_at: Some(oer_time::Instant::from_micros(1_000)),
            ..join_beacon()
        },
        link.bind(&SharedCoex),
    );
    let mut slot = core::pin::pin!(TxSlot::<512>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        station_tsf: 7,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut(), &mut hardware);
    embassy_futures::block_on(tx.wait_until(oer_time::Instant::from_micros(6_000)));
    let mut performed = std::vec::Vec::new();
    settle(
        &mut control,
        &link,
        &mut hardware,
        &mut tx,
        DatapathControlContext::IDLE,
        &mut performed,
    );
    // The power start's write goes through the owner and starts the
    // association's relation.
    assert_eq!(hardware.station_tsf_writes, 1);
    let started = control.station_tsf().generation();

    // A beacon after ten missed ones corrects ten intervals of drift: the
    // relation keeps its generation.
    let interval = 102_400;
    let mut follow =
        |hardware: &mut Hardware, control: &mut ConnectedControl<'_, _, 4>, tx: &mut _, at: u64| {
            // The follow adds the counter's 2 000 µs since the stamp.
            hardware.mac_local_time = 12_000;
            publisher.publish(ConnectedRxEvent::Beacon {
                observation: StaBeaconObservation {
                    timestamp_tsf: oer_ieee80211_mac::tsf::TsfInstant::from_micros(at - 2_000),
                    ..idle_beacon()
                },
                metadata: MacRxMetadata::unavailable(),
                stamp: Some(10_000),
            });
            settle(
                control,
                &link,
                hardware,
                tx,
                DatapathControlContext::IDLE,
                &mut performed,
            );
            assert_eq!(hardware.station_tsf, at);
        };
    hardware.station_tsf += 10 * interval;
    let drifted = hardware.station_tsf + 100;
    follow(&mut hardware, &mut control, &mut tx, drifted);
    assert_eq!(hardware.station_tsf_writes, 2);
    assert_eq!(control.station_tsf().generation(), started);

    // The same correction at the next consecutive beacon is a jump.
    hardware.station_tsf += interval;
    let jumped = hardware.station_tsf + 100;
    follow(&mut hardware, &mut control, &mut tx, jumped);
    assert_eq!(hardware.station_tsf_writes, 3);
    assert_ne!(control.station_tsf().generation(), started);
}

/// A MAC local-time counter that holds still.
#[derive(Clone, Copy)]
struct HeldCounter(u32);

impl crate::mac_clock::LocalTimeCounter for HeldCounter {
    fn read(&self) -> u32 {
        self.0
    }
}

#[test]
fn a_reconnection_starts_a_new_tsf_generation_and_refuses_the_earlier_sample() {
    use oer_ieee80211_lower_mac::{TsfProjectionError, TsfSample, VifId, VifTsf};
    use oer_ieee80211_mac::tsf::TsfInstant;
    let clock: oer_time_virtual::VirtualClock = oer_time_virtual::VirtualClock::new();
    let storage = crate::mac_clock::MacClockStorage::<NoopRawMutex, _, _>::new(&clock);
    let mac = storage.start(HeldCounter(1_000));
    let connect =
        |receiver| ConnectedControl::new(receiver, BSSID, false, tx_block_ack(), mac.tsf_epoch());
    let first_resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (_, first_receiver) = first_resources.split();
    let first = connect(first_receiver);
    let earlier = first.station_tsf().generation();
    let stamp = mac.sample().unwrap().stamp();
    let sample = TsfSample {
        tsf: VifTsf::new(VifId(0), TsfInstant::from_micros(1_000_000)),
        local: stamp,
        uncertainty: oer_ieee80211_lower_mac::Ieee80211Duration::from_micros(1),
        generation: earlier,
    };
    assert!(sample.tsf_at(stamp, earlier).is_ok());

    // The station reconnects: the new association's owner is in a
    // generation the earlier one never had, and the earlier sample does not
    // convert in it.
    drop(first);
    let second_resources = ConnectedControlResources::<NoopRawMutex, 4>::new();
    let (_, second_receiver) = second_resources.split();
    let second = connect(second_receiver);
    let current = second.station_tsf().generation();
    assert_ne!(current, earlier);
    assert_eq!(
        sample.tsf_at(stamp, current),
        Err(TsfProjectionError::StaleSample)
    );
}
