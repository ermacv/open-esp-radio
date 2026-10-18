use super::*;
use crate::modem_sleep::PmCoexAction;

fn core() -> ConnectedControlCore {
    ConnectedControlCore::new(
        [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        true,
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
        .unwrap(),
        1,
    )
}

#[test]
fn readiness_combines_owned_state_with_external_event_state() {
    let mut core = core();
    assert!(!core.has_immediate_work(false));
    assert!(core.has_immediate_work(true));

    core.tx_block_ack
        .queue_initial(oer_ieee80211_mac::block_ack::TxBlockAckRetry {
            attempts: 1,
            interval: oer_time::Duration::ZERO,
        });
    assert!(core.has_immediate_work(false));
}

#[test]
fn deadline_is_computed_without_an_executor_timer() {
    let mut core = core();
    assert_eq!(core.next_alarm_deadline(), None);

    core.tx_block_ack
        .begin(
            7,
            SequenceNumber::new(23).unwrap(),
            oer_time::Instant::from_micros(50),
        )
        .unwrap();
    assert_eq!(
        core.next_alarm_deadline(),
        Some(oer_time::Instant::from_micros(100_050))
    );
}

#[test]
fn connected_ftm_request_is_consumed_at_hardware_frontier() {
    use oer_ieee80211_mac::ftm::{FtmBurstDuration, FtmFormatAndBandwidth, FtmRequestParameters};

    let parameters = FtmRequestParameters::new(
        0,
        FtmBurstDuration::Millis8,
        2,
        None,
        true,
        4,
        FtmFormatAndBandwidth::HtMixed20Mhz,
        0,
    )
    .unwrap();
    let config = FtmRequesterConfig::new(
        parameters,
        oer_time::Duration::from_micros(1_000),
        oer_time::Duration::from_micros(100),
        oer_time::Duration::from_micros(10_000),
        1,
    )
    .unwrap();
    let frontier = core()
        .evaluate_ftm_request_frontier(config, oer_time::Instant::from_micros(50))
        .unwrap();
    assert_eq!(frontier.peer, [0x20, 0x21, 0x22, 0x23, 0x24, 0x25]);
    assert_eq!(frontier.attempt, 1);
    assert_eq!(
        frontier.protocol_event,
        FtmRequesterEvent::Failed(
            oer_ieee80211_sta::ftm::FtmSessionFailure::HardwareAdmissionRejected
        )
    );
    assert_eq!(
        frontier.hardware_error,
        StationFtmHardwareError::Unsupported {
            reached: crate::ftm::StationFtmHardwareStage::PortableInitialRequestValidated,
            missing: crate::ftm::StationFtmUnsupportedStage::RuntimePhyOwnerBinding,
        }
    );
}

#[test]
fn frames_of_blocked_tx_queues_are_not_immediate_work() {
    let mut core = core();
    core.tx_block_ack
        .queue_initial(oer_ieee80211_mac::block_ack::TxBlockAckRetry {
            attempts: 1,
            interval: oer_time::Duration::ZERO,
        });
    core.power.tx_blocked = true;
    assert!(core.power_blocks_tx());
    assert!(!core.admits_network_tx());
    assert!(!core.has_immediate_work(false));
    // A received event is still consumed: it may be a beacon.
    assert!(core.has_immediate_work(true));
}

#[test]
fn power_commands_leave_in_order_and_overflow_is_an_error() {
    let mut core = core();
    for interval in 0..POWER_COMMAND_CAPACITY as u32 {
        core.power
            .push_command(ConnectedPowerCommand::Coex(PmCoexAction::SetInterval(
                interval,
            )))
            .unwrap();
    }
    assert_eq!(
        core.power.push_command(ConnectedPowerCommand::RfWake),
        Err(ConnectedControlError::PowerCommandOverflow)
    );
    for interval in 0..POWER_COMMAND_CAPACITY as u32 {
        assert_eq!(
            core.take_power_command(),
            Some(ConnectedPowerCommand::Coex(PmCoexAction::SetInterval(
                interval
            )))
        );
    }
    assert_eq!(core.take_power_command(), None);
}

#[test]
fn only_invalidating_peer_disconnects_forget_the_pmksa() {
    for reason_code in [2, 6, 7, 15, 49, 50, 51] {
        assert!(ConnectedDisconnectReason::PeerDeauthentication { reason_code }.forgets_pmksa());
        assert!(ConnectedDisconnectReason::PeerDisassociation { reason_code }.forgets_pmksa());
    }
    for reason_code in [1, 3, 4, 8, 14] {
        assert!(!ConnectedDisconnectReason::PeerDeauthentication { reason_code }.forgets_pmksa());
    }
    assert!(!ConnectedDisconnectReason::BeaconLoss.forgets_pmksa());
}

#[derive(Default)]
struct StationTimer {
    tsf: u64,
}

impl StationTsfHardware for StationTimer {
    fn station_tsf(&mut self) -> u64 {
        self.tsf
    }

    fn set_station_tsf(
        &mut self,
        _: oer_esp32s31_ieee80211::station_tsf::StationTsfWrite,
        value: u64,
    ) {
        self.tsf = value;
    }
}

/// A requester whose flow 2 is installed: target 10 000 µs, a 1 024 µs
/// interval, a 256 µs service.
fn active_requester() -> IndividualTwtRequester {
    use oer_ieee80211_mac::twt::{
        IndividualTwtControl, IndividualTwtFlowId, IndividualTwtFlowType,
        IndividualTwtParameterSet, IndividualTwtSetup, IndividualTwtSetupCommand,
    };
    let config = IndividualTwtRequesterConfig::new(
        oer_time::Duration::from_micros(1_000),
        oer_time::Duration::from_micros(100),
        2,
        2,
    )
    .unwrap();
    let parameters = IndividualTwtParameterSet {
        requesting_sta: true,
        setup_command: IndividualTwtSetupCommand::Request,
        trigger: false,
        implicit: true,
        flow_type: IndividualTwtFlowType::Announced,
        flow_id: IndividualTwtFlowId::new(2).unwrap(),
        wake_interval_exponent: 0,
        protection: false,
        target_wake_time_tsf: 10_000,
        nominal_minimum_wake_duration: 1,
        wake_interval_mantissa: 1_024,
        twt_channel: 0,
    };
    let mut requester = IndividualTwtRequester::new(config);
    let start = oer_time::Instant::from_micros(0);
    requester
        .queue_setup(
            IndividualTwtProposal {
                control: IndividualTwtControl::REQUEST,
                parameters,
            },
            start,
        )
        .unwrap();
    let IndividualTwtService::Transmit(setup) = requester.service(start).unwrap() else {
        panic!("setup must be ready");
    };
    requester.complete_transmission(setup, true, start).unwrap();
    let mut accepted = parameters;
    accepted.requesting_sta = false;
    accepted.setup_command = IndividualTwtSetupCommand::Accept;
    let IndividualTwtSetupDisposition::InstallRequired {
        flow_id,
        generation,
        ..
    } = requester
        .on_setup_response(IndividualTwtSetup {
            dialog_token: 1,
            control: IndividualTwtControl::REQUEST,
            parameters: accepted,
        })
        .unwrap()
    else {
        panic!("an implicit accept needs an install");
    };
    requester
        .commit_hardware_install(flow_id, generation)
        .unwrap();
    requester
}

#[test]
fn a_twt_wake_plan_is_stale_after_a_station_tsf_jump() {
    let tsf = oer_ieee80211_mac::tsf::TsfInstant::from_micros;
    let mut core = core();
    let mut timer = StationTimer::default();
    let guard = oer_time::RadioDuration::from_micros(50);
    assert_eq!(core.individual_twt_wake_plan(&mut timer, guard), Ok(None));

    core.individual_twt = Some(active_requester());
    core.station_tsf.set(&mut timer, tsf(12_000)).unwrap();
    let plan = core
        .individual_twt_wake_plan(&mut timer, guard)
        .unwrap()
        .unwrap();
    // 12 000 lies after the window at 11 024: the next one starts at 12 048.
    assert_eq!(plan.plan.service_start, tsf(12_048));
    assert_eq!(plan.plan.wake, tsf(12_000));
    assert_eq!(plan.generation, core.station_tsf().generation());
    assert!(plan.is_current(core.station_tsf()));

    core.station_tsf.set(&mut timer, tsf(500_000)).unwrap();
    assert!(!plan.is_current(core.station_tsf()));
}
