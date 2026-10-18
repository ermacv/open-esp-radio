use oer_ieee802154::{
    AppliedSecurity, AutoPendingMode, Configuration, EnhAckProbing, FrameAddress, LinkMetrics,
    SentAcknowledgement, TxSecurity, TxStatus,
};

use super::{
    RoleCoexPriority, TransmitFailure, csl_period, extended_address, extended_pending_address,
    pending_changes, pending_mode, probing_initiator, psdu_mac, radio_time, role_txrx_priority,
    scan_micros, sent_ack_security, short_pending_address, transmit_failure, tx_security,
    write_applied_security, write_psdu,
};

#[test]
fn psdus_count_the_fcs() {
    assert_eq!(
        psdu_mac(&[0x41, 0x88, 0x01, 0xaa, 0xbb]),
        Some(&[0x41, 0x88, 0x01][..])
    );
    assert_eq!(psdu_mac(&[0xaa, 0xbb]), None);
    assert_eq!(psdu_mac(&[]), None);

    let mut psdu = [0xff; 8];
    assert_eq!(write_psdu(&[1, 2, 3], &mut psdu), Some(5));
    assert_eq!(psdu[..5], [1, 2, 3, 0, 0]);
    assert_eq!(write_psdu(&[1, 2, 3], &mut [0; 4]), None);
}

/// OpenThread's glue packs `otExtAddress.m8` big-endian; over the air the
/// address goes least significant byte first.
#[test]
fn extended_addresses_go_over_the_air_reversed() {
    let address = u64::from_be_bytes([0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
    assert_eq!(
        extended_address(address),
        [0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]
    );
    assert_eq!(
        extended_pending_address(address),
        FrameAddress::Extended([0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11])
    );
    assert_eq!(
        short_pending_address(0x1234),
        FrameAddress::Short([0x34, 0x12])
    );
}

#[test]
fn source_matching_selects_the_pending_mode() {
    assert_eq!(pending_mode(false), AutoPendingMode::Disable);
    assert_eq!(pending_mode(true), AutoPendingMode::Enhanced);
}

/// ESP-IDF's OpenThread port reports a busy channel, an abort and a
/// coexistence rejection as channel-access failures.
#[test]
fn transmit_statuses_map_to_openthread_failures() {
    assert_eq!(transmit_failure(TxStatus::Success), None);
    for status in [
        TxStatus::ChannelBusy,
        TxStatus::Aborted,
        TxStatus::CoexistenceRejected,
    ] {
        assert_eq!(
            transmit_failure(status),
            Some(TransmitFailure::ChannelAccess)
        );
    }
    assert_eq!(
        transmit_failure(TxStatus::NoAcknowledgement),
        Some(TransmitFailure::NoAcknowledgement)
    );
    assert_eq!(
        transmit_failure(TxStatus::InvalidAcknowledgement),
        Some(TransmitFailure::InvalidAcknowledgement)
    );
    assert_eq!(
        transmit_failure(TxStatus::SecurityFailure),
        Some(TransmitFailure::Other)
    );
    assert_eq!(scan_micros(20), 20_000);
}

/// The port's transmit information decides who secures the frame.
#[test]
fn transmit_information_selects_the_security() {
    assert_eq!(tx_security(false, false), TxSecurity::Radio);
    assert_eq!(tx_security(true, false), TxSecurity::Retransmission);
    assert_eq!(tx_security(false, true), TxSecurity::Processed);
    assert_eq!(tx_security(true, true), TxSecurity::Processed);
}

/// The counter and key index the radio wrote land in OpenThread's PSDU at
/// the auxiliary security header, leaving payload and FCS as given.
#[test]
fn applied_security_is_written_into_the_psdu() {
    // 2006 data frame, ENC-MIC-32 in key identifier mode 1, one payload
    // byte, MIC and FCS.
    let mut psdu = [
        0x69, 0x98, 0x07, 0x34, 0x12, 0x02, 0x00, 0x01, 0x00, 0x0d, 0, 0, 0, 0, 0, 0xaa, 1, 2, 3,
        4, 0xf1, 0xf2,
    ];
    let applied = AppliedSecurity {
        frame_counter: 0x0403_0201,
        key_id: Some(7),
    };
    assert!(write_applied_security(applied, &mut psdu));
    assert_eq!(psdu[10..15], [1, 2, 3, 4, 7]);
    assert_eq!(psdu[15..], [0xaa, 1, 2, 3, 4, 0xf1, 0xf2]);

    let mut plain = [
        0x41, 0x98, 0x07, 0x34, 0x12, 0xff, 0xff, 0x02, 0x00, 0xaa, 0, 0,
    ];
    let before = plain;
    assert!(!write_applied_security(applied, &mut plain));
    assert_eq!(plain, before);
}

/// Only a secured enhanced ACK of key identifier mode 1 reports its fields.
#[test]
fn sent_ack_security_needs_a_key_index() {
    let sent = |key_id| SentAcknowledgement {
        frame_pending: true,
        security: Some(AppliedSecurity {
            frame_counter: 9,
            key_id,
        }),
    };
    assert_eq!(sent_ack_security(sent(Some(3))), Some((9, 3)));
    assert_eq!(sent_ack_security(sent(None)), None);
    assert_eq!(sent_ack_security(SentAcknowledgement::NONE), None);
}

/// OpenThread's 32-bit radio times extend to the nearest full instant,
/// across a wrap of the low bits in either direction.
#[test]
fn radio_times_extend_around_now() {
    assert_eq!(radio_time(1_000, 1_500), 1_500);
    assert_eq!(radio_time(1_000, 900), 900);
    let now = (7_u64 << 32) + 10;
    assert_eq!(radio_time(now, 20), now + 10);
    assert_eq!(radio_time(now, u32::MAX - 5), (7_u64 << 32) - 6);
    let late = (7_u64 << 32) + u64::from(u32::MAX) - 10;
    assert_eq!(radio_time(late, 30), (8_u64 << 32) + 30);
}

/// The CSL IE carries 16 bits of the period.
#[test]
fn csl_periods_fit_the_ie() {
    assert_eq!(csl_period(3_125), 3_125);
    assert_eq!(csl_period(0x1_0005), 5);
}

/// Source-match changes remove what left, then add what came, per entry.
#[test]
fn pending_changes_follow_the_table_entry_by_entry() {
    let mut changes = std::vec::Vec::new();
    pending_changes(&[1, 2], &[2, 3], &[10], &[], |change| changes.push(change));
    assert_eq!(
        changes,
        [
            Configuration::RemovePendingAddress(short_pending_address(1)),
            Configuration::RemovePendingAddress(extended_pending_address(10)),
            Configuration::AddPendingAddress(short_pending_address(3)),
        ]
    );
    let mut none = 0;
    pending_changes(&[1], &[1], &[5], &[5], |_| none += 1);
    assert_eq!(none, 0);
}

/// A role change sets the TX/RX priority from the link mode, as
/// `handle_ot_role_change` does.
#[test]
fn role_changes_set_the_txrx_priority_from_the_link_mode() {
    assert_eq!(role_txrx_priority(true), RoleCoexPriority::Low);
    assert_eq!(role_txrx_priority(false), RoleCoexPriority::Middle);
}

/// OpenThread's probing table reaches the radio with extended addresses in
/// frame byte order and its newest initiator matching first; a replaced
/// table forgets the initiators it no longer holds.
#[test]
fn the_probing_table_reaches_the_radio_in_frame_byte_order() {
    let lqi = LinkMetrics {
        lqi: true,
        ..LinkMetrics::NONE
    };
    let rssi = LinkMetrics {
        rssi: true,
        ..LinkMetrics::NONE
    };
    let canonical = [1, 2, 3, 4, 5, 6, 7, 8];
    let frame_order = FrameAddress::Extended([8, 7, 6, 5, 4, 3, 2, 1]);
    let mut probing = EnhAckProbing::<4>::new(-97);
    // Newest first: short 2 was added after short 1, with the same device.
    probing.replace(&[
        probing_initiator(2, canonical, rssi),
        probing_initiator(1, canonical, lqi),
    ]);
    assert_eq!(probing.metrics(frame_order), Some(rssi));
    assert_eq!(probing.metrics(FrameAddress::Short([1, 0])), Some(lqi));

    probing.replace(&[probing_initiator(1, canonical, lqi)]);
    assert_eq!(probing.metrics(FrameAddress::Short([2, 0])), None);
    assert_eq!(probing.metrics(frame_order), Some(lqi));
    assert_eq!(probing.noise_floor(), -97);
}
