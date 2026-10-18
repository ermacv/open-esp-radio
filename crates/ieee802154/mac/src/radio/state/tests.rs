use super::*;
use crate::{
    Configuration, EnergyScanRequest, FcsStatus, FramePending, FrameView, Interface,
    InterfaceSetting, RadioFault, ReceivedFrame, RxMetadata, SecurityStatus, TransmitPowerLevels,
    TxRequest,
};

const ID: RequestId = RequestId::new(7);

fn channel(raw: u8) -> Channel {
    Channel::new(raw).unwrap()
}

fn metadata(channel: Channel) -> RxMetadata {
    RxMetadata {
        channel,
        rssi_dbm: -42,
        link_quality: 211,
        timestamp: None,
        fcs: FcsStatus::Valid,
        security: SecurityStatus::Unprocessed,
        frame_pending: crate::FramePending::Unavailable,
        sent_acknowledgement: Default::default(),
        interface: Some(Interface::PRIMARY),
    }
}

const POWER: TransmitPowerCapability = TransmitPowerCapability {
    levels: match TransmitPowerLevels::new(&[-10, 0, 3, 10]) {
        Ok(levels) => levels,
        Err(_) => panic!("valid levels"),
    },
    reset_dbm: 10,
};

fn enabled(capabilities: RadioCapabilities) -> RadioStateMachine {
    let mut machine = RadioStateMachine::new(capabilities);
    machine.enable().unwrap();
    machine
}

#[test]
fn finite_enable_receive_sleep_disable_path_is_exact() {
    let mut machine = RadioStateMachine::new(RadioCapabilities::NONE);
    assert_eq!(machine.state(), RadioState::Disabled);
    machine.enable().unwrap();
    assert_eq!(machine.enable(), Err(CommandError::AlreadyEnabled));
    assert_eq!(machine.state(), RadioState::Resting(RestingState::Sleeping));

    machine
        .admit(RadioCommand::Receive {
            id: RequestId::new(8),
            channel: channel(15),
        })
        .unwrap();
    assert_eq!(
        machine.state(),
        RadioState::Resting(RestingState::Receiving {
            channel: channel(15)
        })
    );
    machine
        .admit(RadioCommand::Sleep {
            id: RequestId::new(9),
        })
        .unwrap();
    assert_eq!(
        machine.disable(),
        Ok(RadioState::Resting(RestingState::Sleeping))
    );
    assert_eq!(machine.state(), RadioState::Disabled);
    assert_eq!(machine.disable(), Err(CommandError::Disabled));
}

#[test]
fn transmit_correlates_completion_and_restores_receive() {
    let capabilities = RadioCapabilities::CSMA_CA | RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT;
    let mut machine = RadioStateMachine::new(capabilities).with_transmit_power(POWER);
    machine.enable().unwrap();
    machine
        .admit(RadioCommand::Receive {
            id: RequestId::new(1),
            channel: channel(20),
        })
        .unwrap();
    let bytes = [0x61, 0x88, 0x2a];
    let request = TxRequest {
        id: ID,
        frame: FrameView::new(&bytes).unwrap(),
        channel: channel(20),
        mode: TxMode::CsmaCa { max_backoffs: 4 },
        transmit_power_dbm: Some(3),
        max_frame_retries: 0,
        security: Default::default(),
        interface: Interface::PRIMARY,
        time_sync: None,
    };
    machine.admit(RadioCommand::Transmit(request)).unwrap();
    assert_eq!(
        machine.admit(RadioCommand::Sleep {
            id: RequestId::new(99)
        }),
        Err(CommandError::Busy {
            state: machine.state()
        })
    );
    assert_eq!(
        machine.observe(RadioEvent::TransmitDone {
            id: RequestId::new(6),
            status: TxStatus::Success,
            acknowledgement: None,
            security: Default::default(),
        }),
        Err(EventError::RequestMismatch {
            expected: ID,
            actual: RequestId::new(6),
        })
    );
    machine
        .observe(RadioEvent::TransmitDone {
            id: ID,
            status: TxStatus::Success,
            acknowledgement: None,
            security: Default::default(),
        })
        .unwrap();
    assert_eq!(
        machine.state(),
        RadioState::Resting(RestingState::Receiving {
            channel: channel(20)
        })
    );
}

#[test]
fn unsupported_operations_leave_state_unchanged() {
    let mut machine = enabled(RadioCapabilities::NONE);
    let before = machine.state();
    assert_eq!(
        machine.admit(RadioCommand::EnergyScan(EnergyScanRequest {
            id: ID,
            channel: channel(11),
            duration_us: 128,
        })),
        Err(CommandError::Unsupported {
            command: CommandKind::EnergyScan,
            required: RadioCapabilities::ENERGY_SCAN,
        })
    );
    assert_eq!(machine.state(), before);
}

#[test]
fn acknowledgement_capability_is_derived_only_from_the_fcf() {
    let no_ack_bytes = [0x01];
    let mut no_ack_machine = enabled(RadioCapabilities::NONE);
    no_ack_machine
        .admit(RadioCommand::Transmit(TxRequest {
            id: ID,
            frame: FrameView::new(&no_ack_bytes).unwrap(),
            channel: channel(15),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
            max_frame_retries: 0,
            security: Default::default(),
            interface: Interface::PRIMARY,
            time_sync: None,
        }))
        .unwrap();

    let ack_bytes = [0x21];
    let mut ack_machine = enabled(RadioCapabilities::NONE);
    assert_eq!(
        ack_machine.admit(RadioCommand::Transmit(TxRequest {
            id: ID,
            frame: FrameView::new(&ack_bytes).unwrap(),
            channel: channel(15),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
            max_frame_retries: 0,
            security: Default::default(),
            interface: Interface::PRIMARY,
            time_sync: None,
        })),
        Err(CommandError::Unsupported {
            command: CommandKind::Transmit,
            required: RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT,
        })
    );
}

#[test]
fn receive_and_ack_channels_are_checked() {
    let mut machine = enabled(RadioCapabilities::NONE);
    machine
        .admit(RadioCommand::Receive {
            id: ID,
            channel: channel(11),
        })
        .unwrap();
    let bytes = [0x02];
    let received = ReceivedFrame {
        frame: FrameView::new(&bytes).unwrap(),
        metadata: metadata(channel(12)),
    };
    assert_eq!(
        machine.observe(RadioEvent::Received(received)),
        Err(EventError::ChannelMismatch {
            expected: channel(11),
            actual: channel(12),
        })
    );
    assert_eq!(
        machine.state(),
        RadioState::Resting(RestingState::Receiving {
            channel: channel(11)
        })
    );
}

#[test]
fn matching_fault_disables_and_mismatched_fault_is_rejected() {
    let capabilities = RadioCapabilities::ENERGY_SCAN;
    let mut machine = enabled(capabilities);
    machine
        .admit(RadioCommand::EnergyScan(EnergyScanRequest {
            id: ID,
            channel: channel(26),
            duration_us: 64,
        }))
        .unwrap();
    assert_eq!(
        machine.observe(RadioEvent::Fault {
            id: None,
            fault: RadioFault::StateLost,
        }),
        Err(EventError::FaultRequestMismatch {
            expected: Some(ID),
            actual: None,
        })
    );
    machine
        .observe(RadioEvent::Fault {
            id: Some(ID),
            fault: RadioFault::StateLost,
        })
        .unwrap();
    assert_eq!(machine.state(), RadioState::Disabled);
}

#[test]
fn failed_transmit_cannot_publish_an_acknowledgement() {
    let capabilities = RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT;
    let mut machine = enabled(capabilities);
    let frame_bytes = [0x21];
    machine
        .admit(RadioCommand::Transmit(TxRequest {
            id: ID,
            frame: FrameView::new(&frame_bytes).unwrap(),
            channel: channel(15),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
            max_frame_retries: 0,
            security: Default::default(),
            interface: Interface::PRIMARY,
            time_sync: None,
        }))
        .unwrap();
    let ack_bytes = [2];
    let ack = ReceivedFrame {
        frame: FrameView::new(&ack_bytes).unwrap(),
        metadata: RxMetadata {
            frame_pending: FramePending::Clear,
            ..metadata(channel(15))
        },
    };
    assert_eq!(
        machine.observe(RadioEvent::TransmitDone {
            id: ID,
            status: TxStatus::NoAcknowledgement,
            acknowledgement: Some(ack),
            security: Default::default(),
        }),
        Err(EventError::AcknowledgementOnFailedTransmit)
    );
}

/// A backend abort ends an energy scan or a standalone CCA and resumes the
/// resting state; the failure of one operation is not accepted for the other.
#[test]
fn measurement_failures_end_their_own_operation() {
    let capabilities = RadioCapabilities::ENERGY_SCAN | RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT;
    let mut machine = enabled(capabilities);
    machine
        .admit(RadioCommand::EnergyScan(EnergyScanRequest {
            id: RequestId::new(8),
            channel: channel(20),
            duration_us: 128,
        }))
        .unwrap();
    assert!(matches!(
        machine.observe(RadioEvent::ClearChannelAssessmentFailed {
            id: RequestId::new(8)
        }),
        Err(EventError::Unexpected { .. })
    ));
    machine
        .observe(RadioEvent::EnergyScanFailed {
            id: RequestId::new(8),
        })
        .unwrap();
    assert_eq!(machine.state(), RadioState::Resting(RestingState::Sleeping));

    machine
        .admit(RadioCommand::ClearChannelAssessment {
            id: RequestId::new(9),
            channel: channel(20),
        })
        .unwrap();
    machine
        .observe(RadioEvent::ClearChannelAssessmentFailed {
            id: RequestId::new(9),
        })
        .unwrap();
    assert_eq!(machine.state(), RadioState::Resting(RestingState::Sleeping));
}

/// A transmission started from receive mode accepts frames received on its
/// own channel while it waits for the channel; from sleep, or on another
/// channel, reception is a broken sequence.
#[test]
fn a_transmission_from_receive_mode_accepts_frames_on_its_channel() {
    let bytes = [0x02];
    let received = |on: u8| {
        RadioEvent::Received(ReceivedFrame {
            frame: FrameView::new(&bytes).unwrap(),
            metadata: metadata(channel(on)),
        })
    };
    let transmit = |machine: &mut RadioStateMachine| {
        machine
            .admit(RadioCommand::Transmit(TxRequest {
                id: ID,
                frame: FrameView::new(&[0x41, 0x88, 0x01]).unwrap(),
                channel: channel(15),
                mode: TxMode::CsmaCa { max_backoffs: 4 },
                transmit_power_dbm: None,
                max_frame_retries: 0,
                security: Default::default(),
                interface: Interface::PRIMARY,
                time_sync: None,
            }))
            .unwrap();
    };

    let mut machine = enabled(RadioCapabilities::CSMA_CA);
    machine
        .admit(RadioCommand::Receive {
            id: ID,
            channel: channel(11),
        })
        .unwrap();
    transmit(&mut machine);
    let transmitting = machine.state();
    assert_eq!(machine.observe(received(15)), Ok(()));
    assert_eq!(machine.state(), transmitting);
    assert_eq!(
        machine.observe(received(11)),
        Err(EventError::ChannelMismatch {
            expected: channel(15),
            actual: channel(11),
        })
    );

    let mut machine = enabled(RadioCapabilities::CSMA_CA);
    transmit(&mut machine);
    assert!(machine.observe(received(15)).is_err());
}

/// Retransmissions require the retry capability; zero retries does not.
#[test]
fn frame_retries_require_the_retry_capability() {
    let request = |max_frame_retries| {
        RadioCommand::Transmit(TxRequest {
            id: ID,
            frame: FrameView::new(&[0x41, 0x88, 0x01]).unwrap(),
            channel: channel(15),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
            max_frame_retries,
            security: Default::default(),
            interface: Interface::PRIMARY,
            time_sync: None,
        })
    };
    let mut machine = enabled(RadioCapabilities::NONE);
    assert_eq!(
        machine.admit(request(3)),
        Err(CommandError::Unsupported {
            command: CommandKind::Transmit,
            required: RadioCapabilities::TRANSMIT_RETRIES,
        })
    );
    assert!(machine.admit(request(0)).is_ok());
    let mut machine = enabled(RadioCapabilities::TRANSMIT_RETRIES);
    assert!(machine.admit(request(3)).is_ok());
}

fn window(id: u32, on: u8) -> RadioCommand<'static> {
    RadioCommand::ScheduledReceive(crate::ScheduledReceiveRequest {
        id: RequestId::new(id),
        channel: channel(on),
        start: crate::Ieee802154Instant::from_micros(1_000),
        duration_us: 500,
    })
}

/// A scheduled window needs its capability, accepts frames on its channel
/// and sleeps when its own end is reported.
#[test]
fn a_scheduled_receive_window_receives_until_its_end() {
    let mut plain = enabled(RadioCapabilities::NONE);
    assert_eq!(
        plain.admit(window(8, 15)),
        Err(CommandError::Unsupported {
            command: CommandKind::ScheduledReceive,
            required: RadioCapabilities::SCHEDULED_RECEIVE,
        })
    );
    assert_eq!(plain.state(), RadioState::Resting(RestingState::Sleeping));

    let mut machine = enabled(RadioCapabilities::SCHEDULED_RECEIVE);
    machine.admit(window(8, 15)).unwrap();
    let scheduled = RadioState::Resting(RestingState::ScheduledReceiving {
        id: RequestId::new(8),
        channel: channel(15),
    });
    assert_eq!(machine.state(), scheduled);

    let frame = [0x41, 0x88, 0x01];
    let received = |on| {
        RadioEvent::Received(ReceivedFrame {
            frame: FrameView::new(&frame).unwrap(),
            metadata: metadata(channel(on)),
        })
    };
    machine.observe(received(15)).unwrap();
    assert!(matches!(
        machine.observe(received(16)),
        Err(EventError::ChannelMismatch { .. })
    ));
    assert!(matches!(
        machine.observe(RadioEvent::ScheduledReceiveDone {
            id: RequestId::new(9)
        }),
        Err(EventError::RequestMismatch { .. })
    ));
    assert_eq!(machine.state(), scheduled);
    machine
        .observe(RadioEvent::ScheduledReceiveDone {
            id: RequestId::new(8),
        })
        .unwrap();
    assert_eq!(machine.state(), RadioState::Resting(RestingState::Sleeping));
    assert!(matches!(
        machine.observe(RadioEvent::ScheduledReceiveDone {
            id: RequestId::new(8)
        }),
        Err(EventError::Unexpected { .. })
    ));
}

/// An operation admitted during a window ends it: the radio sleeps after
/// the operation, and another command replaces the window.
#[test]
fn an_operation_ends_a_scheduled_window() {
    let mut machine =
        enabled(RadioCapabilities::SCHEDULED_RECEIVE | RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT);
    machine.admit(window(8, 15)).unwrap();
    machine
        .admit(RadioCommand::ClearChannelAssessment {
            id: RequestId::new(9),
            channel: channel(15),
        })
        .unwrap();
    machine
        .observe(RadioEvent::ClearChannelAssessmentDone {
            id: RequestId::new(9),
            idle: true,
        })
        .unwrap();
    assert_eq!(machine.state(), RadioState::Resting(RestingState::Sleeping));

    machine.admit(window(10, 15)).unwrap();
    machine
        .admit(RadioCommand::Receive {
            id: RequestId::new(11),
            channel: channel(20),
        })
        .unwrap();
    assert_eq!(
        machine.state(),
        RadioState::Resting(RestingState::Receiving {
            channel: channel(20)
        })
    );
}

fn configure_interface(index: u8, setting: InterfaceSetting) -> RadioCommand<'static> {
    RadioCommand::Configure {
        id: ID,
        configuration: Configuration::Interface {
            interface: Interface::new(index),
            setting,
        },
    }
}

/// Interface settings need multi-PAN and address only the radio's
/// interfaces; a pending-table setting also needs source matching.
#[test]
fn interface_settings_need_multi_pan_and_an_existing_interface() {
    let mut single = enabled(RadioCapabilities::SOURCE_MATCH);
    assert_eq!(single.interfaces(), 1);
    assert_eq!(
        single.admit(configure_interface(0, InterfaceSetting::PanId(0x1234))),
        Err(CommandError::Unsupported {
            command: CommandKind::Configure,
            required: RadioCapabilities::MULTI_PAN,
        })
    );

    let mut machine = RadioStateMachine::with_interfaces(RadioCapabilities::MULTI_PAN, 4);
    machine.enable().unwrap();
    assert_eq!(machine.interfaces(), 4);
    machine
        .admit(configure_interface(3, InterfaceSetting::Enabled(true)))
        .unwrap();
    assert_eq!(
        machine.admit(configure_interface(4, InterfaceSetting::ShortAddress(1))),
        Err(CommandError::UnknownInterface {
            interface: Interface::new(4),
            interfaces: 4,
        })
    );
    assert_eq!(
        machine.admit(configure_interface(
            1,
            InterfaceSetting::ResetPendingTable(crate::PendingTableHalf::Short)
        )),
        Err(CommandError::Unsupported {
            command: CommandKind::Configure,
            required: RadioCapabilities::MULTI_PAN | RadioCapabilities::SOURCE_MATCH,
        })
    );
    assert_eq!(machine.state(), RadioState::Resting(RestingState::Sleeping));
}

/// Without multi-PAN the interface count stays one whatever was asked.
#[test]
fn a_radio_without_multi_pan_has_the_primary_interface_alone() {
    assert_eq!(
        RadioStateMachine::with_interfaces(RadioCapabilities::NONE, 4).interfaces(),
        1
    );
    assert_eq!(
        RadioStateMachine::with_interfaces(RadioCapabilities::MULTI_PAN, 0).interfaces(),
        1
    );
}

/// A transmission names one of the radio's interfaces.
#[test]
fn a_transmission_names_an_existing_interface() {
    let bytes = [0x41, 0x88, 0x2a];
    let request = |interface| {
        RadioCommand::Transmit(TxRequest {
            id: ID,
            frame: FrameView::new(&bytes).unwrap(),
            channel: channel(20),
            mode: TxMode::Direct,
            transmit_power_dbm: None,
            max_frame_retries: 0,
            security: Default::default(),
            interface: Interface::new(interface),
            time_sync: None,
        })
    };
    let mut single = enabled(RadioCapabilities::NONE);
    assert_eq!(
        single.admit(request(1)),
        Err(CommandError::UnknownInterface {
            interface: Interface::new(1),
            interfaces: 1,
        })
    );
    assert_eq!(single.state(), RadioState::Resting(RestingState::Sleeping));

    let mut machine = RadioStateMachine::with_interfaces(RadioCapabilities::MULTI_PAN, 2);
    machine.enable().unwrap();
    machine.admit(request(1)).unwrap();
}

/// A transmission with a Time IE needs the time-sync capability.
#[test]
fn a_time_ie_needs_the_time_sync_capability() {
    let bytes = [0x41, 0x88, 0x2a];
    let request = RadioCommand::Transmit(TxRequest {
        id: ID,
        frame: FrameView::new(&bytes).unwrap(),
        channel: channel(20),
        mode: TxMode::Direct,
        transmit_power_dbm: None,
        max_frame_retries: 0,
        security: Default::default(),
        interface: Interface::PRIMARY,
        time_sync: Some(crate::TimeSync {
            ie_offset: 0,
            sequence: 1,
            network_time_offset: 0,
        }),
    });
    assert_eq!(
        enabled(RadioCapabilities::NONE).admit(request),
        Err(CommandError::Unsupported {
            command: CommandKind::Transmit,
            required: RadioCapabilities::TIME_SYNC,
        })
    );
    enabled(RadioCapabilities::TIME_SYNC)
        .admit(request)
        .unwrap();
}

/// A cancellation names the running operation, leaves the state to the
/// operation's terminal event, and is refused for anything else.
#[test]
fn cancel_admits_only_the_running_operation() {
    let mut machine = enabled(RadioCapabilities::ENERGY_SCAN);
    // Nothing runs.
    assert_eq!(machine.admit_cancel(ID), Err(CancelError::NotRunning));
    machine
        .admit(RadioCommand::EnergyScan(EnergyScanRequest {
            id: ID,
            channel: channel(11),
            duration_us: 128,
        }))
        .unwrap();
    let scanning = machine.state();
    assert_eq!(
        machine.admit_cancel(RequestId::new(8)),
        Err(CancelError::NotRunning)
    );
    assert_eq!(machine.admit_cancel(ID), Ok(()));
    assert_eq!(machine.state(), scanning);
    machine
        .observe(RadioEvent::EnergyScanFailed { id: ID })
        .unwrap();
    assert_eq!(machine.state(), RadioState::Resting(RestingState::Sleeping));
    assert_eq!(machine.admit_cancel(ID), Err(CancelError::NotRunning));
}

/// An open scheduled receive window can be cancelled; its end event
/// completes it.
#[test]
fn cancel_ends_a_scheduled_receive_window() {
    let mut machine = enabled(RadioCapabilities::SCHEDULED_RECEIVE);
    machine
        .admit(RadioCommand::ScheduledReceive(
            crate::ScheduledReceiveRequest {
                id: ID,
                channel: channel(12),
                start: crate::Ieee802154Instant::from_micros(100),
                duration_us: 1_000,
            },
        ))
        .unwrap();
    assert_eq!(machine.admit_cancel(ID), Ok(()));
    machine
        .observe(RadioEvent::ScheduledReceiveDone { id: ID })
        .unwrap();
    assert_eq!(machine.state(), RadioState::Resting(RestingState::Sleeping));
}

/// A disabled radio runs nothing to cancel.
#[test]
fn a_disabled_radio_has_nothing_to_cancel() {
    let machine = RadioStateMachine::new(RadioCapabilities::NONE);
    assert_eq!(machine.admit_cancel(ID), Err(CancelError::NotRunning));
}

/// A lifecycle terminal confirms a transition the state already took.
#[test]
fn lifecycle_terminals_keep_the_state() {
    let mut machine = enabled(RadioCapabilities::NONE);
    assert_eq!(
        machine.observe(RadioEvent::Lifecycle(crate::LifecycleEvent::Enabled)),
        Ok(())
    );
    assert_eq!(machine.state(), RadioState::Resting(RestingState::Sleeping));
}

fn transmission(transmit_power_dbm: Option<i8>, security: TxSecurity) -> TxRequest<'static> {
    TxRequest {
        id: ID,
        frame: FrameView::new(&[0x41, 0x88, 0x2a]).unwrap(),
        channel: channel(20),
        mode: TxMode::Direct,
        transmit_power_dbm,
        max_frame_retries: 0,
        security,
        interface: Interface::PRIMARY,
        time_sync: None,
    }
}

/// A request's transmit power is checked against the published levels: a
/// power outside them, or any power without them, is refused and the radio
/// stays where it was.
#[test]
fn a_request_power_outside_the_capability_is_refused() {
    let mut machine = RadioStateMachine::new(RadioCapabilities::NONE).with_transmit_power(POWER);
    machine.enable().unwrap();
    assert_eq!(
        machine.admit(RadioCommand::Transmit(transmission(
            Some(11),
            TxSecurity::Radio
        ))),
        Err(CommandError::TransmitPower(
            TransmitPowerError::OutOfRange {
                requested_dbm: 11,
                lowest_dbm: -10,
                highest_dbm: 10,
            }
        ))
    );
    assert_eq!(machine.state(), RadioState::Resting(RestingState::Sleeping));
    machine
        .admit(RadioCommand::Transmit(transmission(
            Some(-10),
            TxSecurity::Radio,
        )))
        .unwrap();

    let mut fixed = enabled(RadioCapabilities::NONE);
    assert_eq!(
        fixed.admit(RadioCommand::Transmit(transmission(
            Some(0),
            TxSecurity::Radio
        ))),
        Err(CommandError::TransmitPower(TransmitPowerError::Unsupported))
    );
    fixed
        .admit(RadioCommand::Transmit(transmission(
            None,
            TxSecurity::Radio,
        )))
        .unwrap();
}

/// A request that carries its own CCM* key needs the security offload.
#[test]
fn a_request_key_needs_the_security_offload() {
    let key = TxSecurity::Key {
        key: [7; 16],
        address: [1; 8],
    };
    let mut machine = enabled(RadioCapabilities::NONE);
    assert_eq!(
        machine.admit(RadioCommand::Transmit(transmission(None, key))),
        Err(CommandError::Unsupported {
            command: CommandKind::Transmit,
            required: RadioCapabilities::SECURITY_OFFLOAD,
        })
    );
    enabled(RadioCapabilities::SECURITY_OFFLOAD)
        .admit(RadioCommand::Transmit(transmission(None, key)))
        .unwrap();
}
