use core::{
    future::Future,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};

use bt_hci::{
    ControllerToHostPacket, PacketKind,
    cmd::{Cmd, SyncCmd, controller_baseband::Reset, le::LeTestEnd},
    controller::{Controller, ExternalController},
    data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
    param::ConnHandle,
    transport::Transport,
};
use embassy_futures::{
    block_on,
    join::{join, join3},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::{HciChannelError, HostToControllerFrame, InProcessHciChannel};
use oer_bluetooth_hci::{
    LE_TEST_END_OPCODE, LeControllerCommandClassification, LeDtmCommand,
    classify_le_controller_command,
};

const RESET_COMMAND_COMPLETE: [u8; 6] = [0x0e, 0x04, 0x01, 0x03, 0x0c, 0x00];
const HARDWARE_ERROR: [u8; 3] = [0x10, 0x01, 0x42];

type TestChannel = InProcessHciChannel<NoopRawMutex, 1, 1, 16>;

#[test]
fn controller_epoch_identity_distinguishes_live_channels() {
    let mut first = TestChannel::new();
    let mut second = TestChannel::new();
    let (_, first_controller) = first.split();
    let (_, second_controller) = second.split();

    let first_identity = first_controller.epoch_identity();
    assert!(first_identity.same_epoch(first_controller.epoch_identity()));
    assert!(!first_identity.same_epoch(second_controller.epoch_identity()));
}

fn command_opcode(
    frame: Result<HostToControllerFrame<'_>, HciChannelError>,
) -> bt_hci::cmd::Opcode {
    match frame {
        Ok(HostToControllerFrame::Command(command)) => command.opcode(),
        other => panic!("expected a command, got {other:?}"),
    }
}

#[test]
fn empty_and_cancelled_receive_consume_nothing() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    let mut buffer = [0; 16];

    assert!(matches!(
        controller.try_receive(&mut buffer),
        Err(HciChannelError::Empty)
    ));
    {
        let mut cancelled = pin!(controller.receive(&mut buffer));
        assert_pending(cancelled.as_mut());
    }
    block_on(host.write(&LeTestEnd::new()))
        .expect("the replacement receive gets one queued command");
    assert_eq!(
        command_opcode(block_on(controller.receive(&mut buffer))),
        LE_TEST_END_OPCODE
    );
    assert!(matches!(
        controller.try_receive(&mut buffer),
        Err(HciChannelError::Empty)
    ));
}

#[test]
fn received_commands_keep_host_fifo_order() {
    type FifoChannel = InProcessHciChannel<NoopRawMutex, 2, 1, 16>;

    let mut channel = FifoChannel::new();
    let (host, controller) = channel.split();
    block_on(async {
        host.write(&LeTestEnd::new()).await.unwrap();
        host.write(&Reset::new()).await.unwrap();
    });

    let mut buffer = [0; 16];
    let Ok(HostToControllerFrame::Command(first)) = controller.try_receive(&mut buffer) else {
        panic!("the oldest Test End comes first");
    };
    assert!(matches!(
        classify_le_controller_command(first),
        LeControllerCommandClassification::Dtm(LeDtmCommand::TestEnd(_))
    ));
    let Ok(HostToControllerFrame::Command(second)) = controller.try_receive(&mut buffer) else {
        panic!("Reset stays second");
    };
    assert!(matches!(
        classify_le_controller_command(second),
        LeControllerCommandClassification::Bootstrap(_)
    ));
}

#[test]
fn publish_readiness_wait_is_side_effect_free_and_wakes_after_drain() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();

    block_on(controller.wait_publish_ready());
    controller
        .try_publish(PacketKind::Event, &HARDWARE_ERROR)
        .unwrap();

    let mut wait = pin!(controller.wait_publish_ready());
    assert_pending(wait.as_mut());
    let mut event_buffer = [0; 16];
    block_on(host.read::<ControllerToHostPacket<'_>>(&mut event_buffer)).unwrap();
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(wait.as_mut().poll(&mut context), Poll::Ready(())));

    controller
        .try_publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
        .expect("readiness did not manufacture or reserve a packet");
}

#[test]
fn receive_readiness_wait_is_side_effect_free_and_wakes_after_publish() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();

    {
        let mut wait = pin!(controller.wait_receive_ready());
        assert_pending(wait.as_mut());
        block_on(host.write(&LeTestEnd::new())).unwrap();
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(wait.as_mut().poll(&mut context), Poll::Ready(())));
    }

    let mut buffer = [0; 16];
    assert_eq!(
        command_opcode(controller.try_receive(&mut buffer)),
        LE_TEST_END_OPCODE
    );
    assert!(matches!(
        controller.try_receive(&mut buffer),
        Err(HciChannelError::Empty)
    ));
}

#[test]
fn cancelled_receive_readiness_wait_consumes_and_reserves_nothing() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();

    {
        let mut cancelled = pin!(controller.wait_receive_ready());
        assert_pending(cancelled.as_mut());
    }

    block_on(host.write(&LeTestEnd::new())).unwrap();
    block_on(controller.wait_receive_ready());
    block_on(controller.wait_receive_ready());

    let mut buffer = [0; 16];
    assert_eq!(
        command_opcode(controller.try_receive(&mut buffer)),
        LE_TEST_END_OPCODE
    );
}

#[test]
fn receive_transfers_the_exact_data_frame() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    let acl = AclPacket::new(
        ConnHandle::new(7),
        AclPacketBoundary::Complete,
        AclBroadcastFlag::PointToPoint,
        &[11, 13],
    );
    block_on(host.write(&acl)).unwrap();

    let mut storage = [0; 16];
    let Ok(HostToControllerFrame::Acl(received)) = controller.try_receive(&mut storage) else {
        panic!("the outer router must receive the original ACL kind");
    };
    assert_eq!(received.handle(), ConnHandle::new(7));
    assert_eq!(received.data(), &[11, 13]);
}

#[test]
fn cancelled_publish_readiness_wait_leaves_capacity_for_replacement() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    controller
        .try_publish(PacketKind::Event, &HARDWARE_ERROR)
        .unwrap();

    {
        let mut cancelled = pin!(controller.wait_publish_ready());
        assert_pending(cancelled.as_mut());
    }

    let mut event_buffer = [0; 16];
    block_on(host.read::<ControllerToHostPacket<'_>>(&mut event_buffer)).unwrap();
    block_on(controller.wait_publish_ready());
    controller
        .try_publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
        .expect("cancelled readiness did not consume the released slot");
}

#[test]
fn typed_reset_and_event_cross_the_direct_hci_boundary() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();

    block_on(async {
        host.write(&Reset::new()).await.unwrap();
        let mut command_buffer = [0; 16];
        let HostToControllerFrame::Command(command) =
            controller.receive(&mut command_buffer).await.unwrap()
        else {
            panic!("Reset changed packet kind");
        };
        assert_eq!(command.opcode(), Reset::OPCODE);
        assert!(command.parameters().is_empty());
        assert!(controller.host_to_controller.vacant_storage_is_zeroed());

        controller
            .publish(PacketKind::Event, &HARDWARE_ERROR)
            .await
            .unwrap();
        let mut event_buffer = [0; 16];
        let ControllerToHostPacket::Event(event) = host.read(&mut event_buffer).await.unwrap()
        else {
            panic!("Hardware Error changed packet kind");
        };
        assert_eq!(event.data, &[0x42]);
        assert!(controller.controller_to_host.vacant_storage_is_zeroed());
    });
}

#[test]
fn external_controller_completes_a_command_via_the_same_event_loop() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    let external = ExternalController::<_, 1>::new(host);

    block_on(async {
        let reset = Reset::new();
        let mut event_buffer = external.alloc_buf().unwrap();
        let worker = async {
            let mut command_buffer = [0; 16];
            let HostToControllerFrame::Command(command) =
                controller.receive(&mut command_buffer).await.unwrap()
            else {
                panic!("Reset changed packet kind");
            };
            assert_eq!(command.opcode(), Reset::OPCODE);
            controller
                .publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
                .await
                .unwrap();
            controller
                .publish(PacketKind::Event, &HARDWARE_ERROR)
                .await
                .unwrap();
        };

        let (completed, received, ()) = join3(
            reset.exec(&external),
            external.read(&mut event_buffer),
            worker,
        )
        .await;
        completed.unwrap();
        assert!(matches!(
            received.unwrap(),
            ControllerToHostPacket::Event(_)
        ));
    });
}

#[test]
fn both_async_directions_wake_without_polling_or_an_rtos() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();

    block_on(async {
        let mut command_buffer = [0; 16];
        let (received, sent) = join(
            controller.receive(&mut command_buffer),
            host.write(&Reset::new()),
        )
        .await;
        sent.unwrap();
        assert!(matches!(
            received.unwrap(),
            HostToControllerFrame::Command(_)
        ));

        let mut event_buffer = [0; 16];
        let (received, sent) = join(
            host.read(&mut event_buffer),
            controller.publish(PacketKind::Event, &HARDWARE_ERROR),
        )
        .await;
        sent.unwrap();
        assert!(matches!(
            received.unwrap(),
            ControllerToHostPacket::Event(_)
        ));
    });
}

#[test]
fn cancelled_backpressure_waits_never_publish_a_packet() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    let reset = Reset::new();

    block_on(async {
        host.write(&reset).await.unwrap();
        {
            let mut second_write = pin!(host.write(&reset));
            assert_pending(second_write.as_mut());
        }

        let mut command_buffer = [0; 16];
        assert!(controller.receive(&mut command_buffer).await.is_ok());
        assert!(matches!(
            controller.try_receive(&mut command_buffer),
            Err(HciChannelError::Empty)
        ));

        controller
            .publish(PacketKind::Event, &HARDWARE_ERROR)
            .await
            .unwrap();
        {
            let mut second_publish =
                pin!(controller.publish(PacketKind::Event, &RESET_COMMAND_COMPLETE));
            assert_pending(second_publish.as_mut());
        }

        let mut event_buffer = [0; 16];
        assert!(
            host.read::<ControllerToHostPacket<'_>>(&mut event_buffer)
                .await
                .is_ok()
        );
        controller
            .try_publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
            .unwrap();
        assert!(
            host.read::<ControllerToHostPacket<'_>>(&mut event_buffer)
                .await
                .is_ok()
        );
    });
}

#[test]
fn short_profile_buffers_fail_before_consuming_either_direction() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();

    block_on(async {
        host.write(&Reset::new()).await.unwrap();
        let mut short = [0; 15];
        assert!(matches!(
            controller.receive(&mut short).await,
            Err(HciChannelError::DestinationTooSmall {
                required: 16,
                available: 15,
            })
        ));
        let mut complete = [0; 16];
        assert!(controller.receive(&mut complete).await.is_ok());

        controller
            .publish(PacketKind::Event, &HARDWARE_ERROR)
            .await
            .unwrap();
        assert!(matches!(
            host.read::<ControllerToHostPacket<'_>>(&mut short).await,
            Err(HciChannelError::DestinationTooSmall {
                required: 16,
                available: 15,
            })
        ));
        assert!(
            host.read::<ControllerToHostPacket<'_>>(&mut complete)
                .await
                .is_ok()
        );
    });
}

#[test]
fn try_publication_rejects_direction_length_and_overwrite() {
    let mut channel = TestChannel::new();
    let (_host, controller) = channel.split();

    assert_eq!(
        controller.try_publish(PacketKind::Cmd, &[0x03, 0x0c, 0x00]),
        Err(HciChannelError::InvalidDirection)
    );
    assert_eq!(
        controller.try_publish(PacketKind::Event, &[0x10, 0x00, 0xff]),
        Err(HciChannelError::TrailingBytes)
    );
    controller
        .try_publish(PacketKind::Event, &HARDWARE_ERROR)
        .unwrap();
    assert_eq!(
        controller.try_publish(PacketKind::Event, &RESET_COMMAND_COMPLETE),
        Err(HciChannelError::Full)
    );
}

#[test]
fn async_queue_preserves_fifo_across_ring_wrap() {
    type RingChannel = InProcessHciChannel<NoopRawMutex, 2, 2, 16>;
    let mut channel = RingChannel::new();
    let (host, controller) = channel.split();
    let first = [0x10, 0x01, 0x11];
    let second = [0x10, 0x01, 0x22];
    let third = [0x10, 0x01, 0x33];

    block_on(async {
        controller.publish(PacketKind::Event, &first).await.unwrap();
        controller
            .publish(PacketKind::Event, &second)
            .await
            .unwrap();
        let mut buffer = [0; 16];
        assert_eq!(event_parameter(host.read(&mut buffer).await.unwrap()), 0x11);
        controller.publish(PacketKind::Event, &third).await.unwrap();
        assert_eq!(event_parameter(host.read(&mut buffer).await.unwrap()), 0x22);
        assert_eq!(event_parameter(host.read(&mut buffer).await.unwrap()), 0x33);
        assert!(controller.controller_to_host.vacant_storage_is_zeroed());
    });
}

#[test]
fn every_host_packet_header_rejects_declared_length_mismatch() {
    for (kind, bytes) in [
        (PacketKind::Cmd, &[0x03, 0x0c, 0x00, 0xaa][..]),
        (PacketKind::AclData, &[0x01, 0x00, 0x00, 0x00, 0xaa][..]),
        (PacketKind::SyncData, &[0x01, 0x00, 0x00, 0xbb][..]),
        (PacketKind::IsoData, &[0x01, 0x00, 0x00, 0x00, 0xcc][..]),
    ] {
        assert_eq!(
            super::codec::validate_host_packet(kind, bytes),
            Err(HciChannelError::TrailingBytes)
        );
    }
    assert_eq!(
        super::codec::validate_host_packet(PacketKind::Cmd, &[0x03, 0x0c, 0x01]),
        Err(HciChannelError::InvalidPacket(
            bt_hci::FromHciBytesError::InvalidSize
        ))
    );
    assert_eq!(
        super::codec::validate_host_packet(PacketKind::Event, &HARDWARE_ERROR),
        Err(HciChannelError::InvalidDirection)
    );
}

fn event_parameter(packet: ControllerToHostPacket<'_>) -> u8 {
    let ControllerToHostPacket::Event(event) = packet else {
        panic!("event changed packet kind");
    };
    event.data[0]
}

fn assert_pending<F: Future>(mut future: Pin<&mut F>) {
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
}

#[test]
fn terminal_close_preserves_both_fifos_then_reports_closed() {
    let mut channel = InProcessHciChannel::<NoopRawMutex, 2, 2, 16>::new();
    let (host, controller) = channel.split();
    block_on(host.write(&Reset::new())).unwrap();
    block_on(host.write(&LeTestEnd::new())).unwrap();
    controller
        .try_publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
        .unwrap();
    controller
        .try_publish(PacketKind::Event, &HARDWARE_ERROR)
        .unwrap();
    controller.close();
    controller.close();
    assert_eq!(
        block_on(host.write(&Reset::new())),
        Err(HciChannelError::Closed)
    );
    assert_eq!(
        controller.try_publish(PacketKind::Event, &HARDWARE_ERROR),
        Err(HciChannelError::Closed)
    );
    let mut buffer = [0; 16];
    for opcode in [Reset::OPCODE, LeTestEnd::OPCODE] {
        let HostToControllerFrame::Command(command) = controller.try_receive(&mut buffer).unwrap()
        else {
            panic!("queued command changed kind");
        };
        assert_eq!(command.opcode(), opcode);
    }
    assert!(matches!(
        controller.try_receive(&mut buffer),
        Err(HciChannelError::Closed)
    ));
    let first: ControllerToHostPacket<'_> = block_on(host.read(&mut buffer)).unwrap();
    assert!(
        matches!(first, ControllerToHostPacket::Event(event) if event.kind == bt_hci::event::EventKind::CommandComplete)
    );
    assert_eq!(
        event_parameter(block_on(host.read(&mut buffer)).unwrap()),
        0x42
    );
    assert!(matches!(
        block_on(host.read::<ControllerToHostPacket<'_>>(&mut buffer)),
        Err(HciChannelError::Closed)
    ));
    assert!(channel.host_to_controller.vacant_storage_is_zeroed());
    assert!(channel.controller_to_host.vacant_storage_is_zeroed());
    assert!(!channel.is_pristine());
}

#[derive(Default)]
struct CloseWake(std::sync::atomic::AtomicUsize);

impl std::task::Wake for CloseWake {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn terminal_close_wakes_blocked_host_write_and_read_without_accepting_write() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    block_on(host.write(&Reset::new())).unwrap();
    let write_wake = std::sync::Arc::new(CloseWake::default());
    let read_wake = std::sync::Arc::new(CloseWake::default());
    let write_waker = Waker::from(write_wake.clone());
    let read_waker = Waker::from(read_wake.clone());
    let mut write_context = Context::from_waker(&write_waker);
    let mut read_context = Context::from_waker(&read_waker);
    let command = LeTestEnd::new();
    let mut buffer = [0; 16];
    let mut write = pin!(host.write(&command));
    let mut read = pin!(host.read::<ControllerToHostPacket<'_>>(&mut buffer));
    assert!(write.as_mut().poll(&mut write_context).is_pending());
    assert!(read.as_mut().poll(&mut read_context).is_pending());
    controller.close();
    assert!(write_wake.0.load(std::sync::atomic::Ordering::SeqCst) > 0);
    assert!(read_wake.0.load(std::sync::atomic::Ordering::SeqCst) > 0);
    assert_eq!(
        write.as_mut().poll(&mut write_context),
        Poll::Ready(Err(HciChannelError::Closed))
    );
    assert!(matches!(
        read.as_mut().poll(&mut read_context),
        Poll::Ready(Err(HciChannelError::Closed))
    ));
    let mut retained = [0; 16];
    assert!(
        matches!(controller.try_receive(&mut retained), Ok(HostToControllerFrame::Command(command)) if command.opcode() == Reset::OPCODE)
    );
    assert!(matches!(
        controller.try_receive(&mut retained),
        Err(HciChannelError::Closed)
    ));
}

#[test]
fn closed_readiness_and_cancelled_waits_observe_terminal_state() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    controller
        .try_publish(PacketKind::Event, &HARDWARE_ERROR)
        .unwrap();
    {
        let mut read_ready = pin!(controller.wait_receive_ready());
        let mut write_ready = pin!(controller.wait_publish_ready());
        assert_pending(read_ready.as_mut());
        assert_pending(write_ready.as_mut());
        controller.close();
        // Cancel both readiness futures before they observe the notification.
    }
    block_on(controller.wait_receive_ready());
    block_on(controller.wait_publish_ready());
    assert_eq!(
        controller.try_publish(PacketKind::Event, &HARDWARE_ERROR),
        Err(HciChannelError::Closed)
    );
    let mut buffer = [0; 16];
    assert!(matches!(
        controller.try_receive(&mut buffer),
        Err(HciChannelError::Closed)
    ));
    assert_eq!(
        event_parameter(block_on(host.read(&mut buffer)).unwrap()),
        0x42
    );
}

#[test]
fn closed_epoch_rejects_independent_host_acl_credit_sender() {
    let mut channel = TestChannel::new();
    let (host, controller) = channel.split();
    let credits = host.acl_credit_sender();
    block_on(host.write(&Reset::new())).unwrap();
    let mut pending = pin!(credits.return_completed_packets(&[]));
    assert_pending(pending.as_mut());
    controller.close();
    assert_eq!(block_on(pending), Err(HciChannelError::Closed));
    assert_eq!(
        block_on(credits.return_completed_packets(&[])),
        Err(HciChannelError::Closed)
    );
}

#[test]
fn retirement_requires_drained_queues_and_restart_needs_its_own_proof() {
    use super::{HciRestartError, HciRetirementError};

    let mut channel = TestChannel::new();
    let mut other = TestChannel::new();
    let (host, mut controller) = channel.split();
    let (_, other_controller) = other.split();

    block_on(host.write(&Reset::new())).unwrap();
    assert!(matches!(
        controller.try_retire(),
        Err(HciRetirementError::HostPacketsPending)
    ));
    let mut buffer = [0; 16];
    controller.try_receive(&mut buffer).unwrap();
    controller
        .try_publish(PacketKind::Event, &HARDWARE_ERROR)
        .unwrap();
    assert!(matches!(
        controller.try_retire(),
        Err(HciRetirementError::ControllerPacketsPending)
    ));
    block_on(host.read::<ControllerToHostPacket<'_>>(&mut buffer)).unwrap();

    let foreign = other_controller.try_retire().unwrap();
    let retired = controller.try_retire().unwrap();
    assert!(retired.matches_epoch(controller.epoch_identity()));
    let Err((error, _)) = controller.restart(foreign) else {
        panic!("a foreign proof cannot restart this epoch");
    };
    assert_eq!(error, HciRestartError::EpochMismatch);
    // The old Host stays closed.
    assert_eq!(
        block_on(host.write(&Reset::new())),
        Err(HciChannelError::Closed)
    );
    let old_identity = controller.epoch_identity();
    let Ok(new_host) = controller.restart(retired) else {
        panic!("the matching proof restarts the epoch");
    };
    assert!(!old_identity.same_epoch(controller.epoch_identity()));
    block_on(new_host.write(&Reset::new())).unwrap();
    assert_eq!(
        block_on(host.write(&Reset::new())),
        Err(HciChannelError::Closed)
    );
}
