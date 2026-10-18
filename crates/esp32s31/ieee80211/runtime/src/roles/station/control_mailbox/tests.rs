use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_esp32s31_ieee80211_mac::tx::ampdu::BlockAckAction;
use oer_esp32s31_ieee80211_sta::connected_rx::{
    ConnectedRxControlEvent, ConnectedRxEvent, ConnectedRxSink, PowerSaveData,
};
use oer_ieee80211_mac::{
    data::EthernetFrameParts,
    station::{StaDisconnect, StaDisconnectKind},
};

use super::*;

#[test]
fn synchronous_queue_copies_actions_but_never_borrowed_ethernet() {
    let body = [3, 2, 0, 0, 0, 0];
    let action = BlockAckAction::Delba {
        tid: 0,
        initiator: true,
        reason: 37,
    };
    let mut queue = ConnectedControlQueue::<1>::new();

    queue.publish(ConnectedRxEvent::Ethernet {
        frame: EthernetFrameParts {
            destination: [0; 6],
            source: [0; 6],
            ether_type: 0,
            payload: &[],
        },
        raw: &[0; 14],
        amsdu: false,
        metadata: oer_ieee80211_softmac::MacRxMetadata::unavailable(),
    });
    queue.publish(ConnectedRxEvent::BlockAck {
        action,
        body: &body,
    });
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.dropped(), 0);
    assert_eq!(queue.pop(), Some(ConnectedRxControlEvent::BlockAck(action)));
    assert!(queue.is_empty());
}

#[test]
fn embassy_endpoints_preserve_fifo_and_report_overflow() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (mut publisher, receiver) = resources.split();
    let first = BlockAckAction::Delba {
        tid: 1,
        initiator: true,
        reason: 2,
    };
    let second = BlockAckAction::Delba {
        tid: 3,
        initiator: false,
        reason: 4,
    };

    publisher.publish(ConnectedRxEvent::BlockAck {
        action: first,
        body: &[3, 2, 0, 0, 2, 0],
    });
    publisher.publish(ConnectedRxEvent::BlockAck {
        action: second,
        body: &[3, 2, 0, 0, 4, 0],
    });

    assert_eq!(receiver.len(), 1);
    assert!(receiver.overflowed());
    assert_eq!(
        receiver.try_receive(),
        Some(ConnectedRxControlEvent::BlockAck(first))
    );
    assert_eq!(receiver.try_receive(), None);
}

#[test]
fn power_save_data_is_published_only_while_armed() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (mut publisher, receiver) = resources.split();
    let data = PowerSaveData {
        group: false,
        more_data: true,
    };
    assert!(!publisher.wants_power_save_data());
    publisher.publish(ConnectedRxEvent::PowerSaveData(data));
    assert_eq!(receiver.try_receive_power_save_data(), None);

    receiver.set_power_save_data_armed(true);
    assert!(publisher.wants_power_save_data());
    publisher.publish(ConnectedRxEvent::PowerSaveData(data));
    assert_eq!(
        receiver.try_receive_power_save_data(),
        Some(ConnectedRxControlEvent::PowerSaveData(data))
    );

    publisher.publish(ConnectedRxEvent::PowerSaveData(data));
    receiver.set_power_save_data_armed(false);
    assert!(!publisher.wants_power_save_data());
    assert_eq!(receiver.try_receive_power_save_data(), None);
}

#[test]
fn peer_disconnect_has_a_reserved_terminal_slot_and_priority() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (mut publisher, receiver) = resources.split();
    let action = BlockAckAction::Delba {
        tid: 1,
        initiator: true,
        reason: 2,
    };
    let disconnect = StaDisconnect {
        kind: StaDisconnectKind::Disassociation,
        reason_code: 8,
    };

    publisher.publish(ConnectedRxEvent::BlockAck {
        action,
        body: &[3, 2, 0, 0, 2, 0],
    });
    publisher.publish(ConnectedRxEvent::PeerDisconnect(disconnect));

    assert_eq!(receiver.len(), 2);
    assert!(!receiver.overflowed());
    assert_eq!(
        receiver.try_receive(),
        Some(ConnectedRxControlEvent::PeerDisconnect(disconnect))
    );
    assert_eq!(
        receiver.try_receive(),
        Some(ConnectedRxControlEvent::BlockAck(action))
    );
}

#[test]
fn connected_eapol_uses_the_security_lane_and_never_the_control_fifo() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (mut publisher, receiver) = resources.split();
    let ap = [2, 0, 0, 0, 0, 2];
    let packet = oer_ieee80211_rsn::frames::RsnTxFrame::<512>::group_message1(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        [2, 0, 0, 0, 0, 1],
        3,
        [0; 8],
        &[0x55; 24],
    )
    .unwrap();

    publisher.publish(ConnectedRxEvent::Ethernet {
        frame: EthernetFrameParts {
            destination: [2, 0, 0, 0, 0, 1],
            source: ap,
            ether_type: EAPOL_ETHERTYPE,
            payload: packet.as_bytes(),
        },
        raw: &[],
        amsdu: false,
        metadata: oer_ieee80211_softmac::MacRxMetadata::unavailable(),
    });

    assert_eq!(receiver.try_receive(), None);
    let ConnectedSecurityFrame::Protected(received) = receiver
        .try_receive_security()
        .expect("EAPOL must use the security lane")
    else {
        panic!("Ethernet EAPOL must retain protected provenance")
    };
    assert_eq!(received.peer(), &ap);
    assert_eq!(received.as_bytes(), packet.as_bytes());
    assert!(!receiver.overflowed());
}

#[test]
fn protected_eapol_overflow_remains_fail_closed() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (mut publisher, receiver) = resources.split();
    let ap = [2, 0, 0, 0, 0, 2];
    let packet = oer_ieee80211_rsn::frames::RsnTxFrame::<512>::group_message1(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        [2, 0, 0, 0, 0, 1],
        3,
        [0; 8],
        &[0x55; 24],
    )
    .unwrap();
    let event = || ConnectedRxEvent::Ethernet {
        frame: EthernetFrameParts {
            destination: [2, 0, 0, 0, 0, 1],
            source: ap,
            ether_type: EAPOL_ETHERTYPE,
            payload: packet.as_bytes(),
        },
        raw: &[],
        amsdu: false,
        metadata: oer_ieee80211_softmac::MacRxMetadata::unavailable(),
    };

    publisher.publish(event());
    publisher.publish(event());

    assert!(receiver.overflowed());
    let ConnectedSecurityFrame::Protected(received) = receiver
        .try_receive_security()
        .expect("first protected EAPOL must remain queued")
    else {
        panic!("protected lane must retain its provenance")
    };
    assert_eq!(received.as_bytes(), packet.as_bytes());
}

#[test]
fn plaintext_eapol_full_and_copy_rejection_are_peer_local_drops() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (mut publisher, receiver) = resources.split();
    let ap = [2, 0, 0, 0, 0, 2];
    let first = oer_ieee80211_rsn::frames::RsnTxFrame::<512>::message3(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        [2, 0, 0, 0, 0, 1],
        3,
        [4; 32],
        [0; 8],
        &[0x55; 8],
    )
    .unwrap();
    let second = oer_ieee80211_rsn::frames::RsnTxFrame::<512>::message3(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        [2, 0, 0, 0, 0, 1],
        4,
        [5; 32],
        [0; 8],
        &[0x66; 8],
    )
    .unwrap();

    publisher.publish(ConnectedRxEvent::UnprotectedEapol {
        source: ap,
        payload: first.as_bytes(),
    });
    publisher.publish(ConnectedRxEvent::UnprotectedEapol {
        source: ap,
        payload: second.as_bytes(),
    });

    assert!(!receiver.overflowed());
    let ConnectedSecurityFrame::Unprotected(received) = receiver
        .try_receive_security()
        .expect("first plaintext EAPOL must remain queued")
    else {
        panic!("plaintext EAPOL must retain unprotected provenance")
    };
    assert_eq!(received.peer(), &ap);
    assert_eq!(received.as_bytes(), first.as_bytes());
    assert!(receiver.try_receive_security().is_none());

    publisher.publish(ConnectedRxEvent::UnprotectedEapol {
        source: ap,
        payload: &[0],
    });
    assert!(!receiver.overflowed());
    assert!(receiver.try_receive_security().is_none());
}

#[test]
fn protected_security_precedes_an_earlier_plaintext_candidate() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let (mut publisher, receiver) = resources.split();
    let station = [2, 0, 0, 0, 0, 1];
    let ap = [2, 0, 0, 0, 0, 2];
    let message3 = oer_ieee80211_rsn::frames::RsnTxFrame::<512>::message3(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        station,
        3,
        [4; 32],
        [0; 8],
        &[0x55; 8],
    )
    .unwrap();
    let group_message1 = oer_ieee80211_rsn::frames::RsnTxFrame::<512>::group_message1(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        station,
        4,
        [0; 8],
        &[0x66; 24],
    )
    .unwrap();

    publisher.publish(ConnectedRxEvent::UnprotectedEapol {
        source: ap,
        payload: message3.as_bytes(),
    });
    publisher.publish(ConnectedRxEvent::Ethernet {
        frame: EthernetFrameParts {
            destination: station,
            source: ap,
            ether_type: EAPOL_ETHERTYPE,
            payload: group_message1.as_bytes(),
        },
        raw: &[],
        amsdu: false,
        metadata: oer_ieee80211_softmac::MacRxMetadata::unavailable(),
    });

    assert!(!receiver.overflowed());
    let ConnectedSecurityFrame::Protected(protected) = receiver
        .try_receive_security()
        .expect("protected lane must have dequeue priority")
    else {
        panic!("first dequeued security frame must be protected")
    };
    assert_eq!(protected.as_bytes(), group_message1.as_bytes());
    let ConnectedSecurityFrame::Unprotected(unprotected) = receiver
        .try_receive_security()
        .expect("plaintext candidate must remain independently queued")
    else {
        panic!("second dequeued security frame must be unprotected")
    };
    assert_eq!(unprotected.as_bytes(), message3.as_bytes());
}

#[test]
fn resources_are_reused_by_sequential_epochs() {
    let resources = ConnectedControlResources::<NoopRawMutex, 1>::new();
    let action = BlockAckAction::Delba {
        tid: 2,
        initiator: true,
        reason: 8,
    };

    {
        let (mut publisher, receiver) = resources.split();
        publisher.publish(ConnectedRxEvent::BlockAck {
            action,
            body: &[3, 2, 0, 0, 8, 0],
        });
        assert_eq!(
            receiver.try_receive(),
            Some(ConnectedRxControlEvent::BlockAck(action))
        );
        publisher.publish(ConnectedRxEvent::BlockAck {
            action,
            body: &[3, 2, 0, 0, 8, 0],
        });
        publisher.publish(ConnectedRxEvent::BlockAck {
            action,
            body: &[3, 2, 0, 0, 8, 0],
        });
        assert!(receiver.overflowed());
        assert_eq!(
            receiver.try_receive(),
            Some(ConnectedRxControlEvent::BlockAck(action))
        );
    }

    let (mut publisher, receiver) = resources.split();
    assert!(!receiver.overflowed());
    publisher.publish(ConnectedRxEvent::BlockAck {
        action,
        body: &[3, 2, 0, 0, 8, 0],
    });
    assert_eq!(
        receiver.try_receive(),
        Some(ConnectedRxControlEvent::BlockAck(action))
    );
}
