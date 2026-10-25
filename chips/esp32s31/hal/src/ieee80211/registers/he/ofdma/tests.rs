use super::*;

#[test]
fn traffic_identifier_limits_expose_semantic_membership() {
    let tids: [MacHeTid; MacHeTid::COUNT] =
        core::array::from_fn(|value| MacHeTid::new(value as u8).unwrap());

    assert!(MacHeTbTidLimit::One.contains(tids[0]));
    assert!(!MacHeTbTidLimit::One.contains(tids[1]));
    assert!(MacHeTbTidLimit::Two.contains(tids[0]));
    assert!(MacHeTbTidLimit::Two.contains(tids[7]));
    assert!(!MacHeTbTidLimit::Two.contains(tids[5]));
    assert!(MacHeTbTidLimit::Three.contains(tids[0]));
    assert!(MacHeTbTidLimit::Three.contains(tids[5]));
    assert!(MacHeTbTidLimit::Three.contains(tids[7]));
    assert!(MacHeTbTidLimit::Four.contains(tids[1]));
    assert_eq!(MacHeTid::new(8), None);
    assert_eq!(MacHeTid::new(u8::MAX), None);
    assert_eq!(MacHeTbTidLimit::default(), MacHeTbTidLimit::Three);
}

#[test]
fn beamforming_average_snr_retains_the_blob_quarter_db_transform() {
    assert_eq!(
        MacBeamformingAverageSnr::from_raw_code(0),
        MacBeamformingAverageSnr {
            raw_code: 0,
            quarter_db: 88,
            is_lower_bound: false,
        }
    );
    assert_eq!(
        MacBeamformingAverageSnr::from_raw_code(0x7f),
        MacBeamformingAverageSnr {
            raw_code: 0x7f,
            quarter_db: 215,
            is_lower_bound: true,
        }
    );
}

#[test]
fn default_trigger_link_ranges_partition_all_120_entries() {
    let expected = [(32, 32), (64, 32), (0, 32), (96, 24)];
    let mut seen = [false; 120];
    for (queue, (first, capacity)) in expected.into_iter().enumerate() {
        let reservation =
            MacHeTbLinkReservation::for_queue(MacHeTbTidLimit::Three, queue as u8, capacity)
                .unwrap();
        assert_eq!(reservation.first(), first);
        assert_eq!(reservation.count(), capacity);
        for position in 0..capacity {
            let index = reservation.index(position).unwrap();
            assert!(!seen[usize::from(index)]);
            seen[usize::from(index)] = true;
            assert_eq!(
                reservation.next(position).unwrap(),
                if position + 1 == capacity {
                    oer_esp32s31_pac::wifi_mac::he_scratch::NextLink::End.bits() as u8
                } else {
                    index + 1
                }
            );
        }
    }
    assert!(seen.into_iter().all(core::convert::identity));
}

#[test]
fn single_tid_trigger_link_ranges_match_the_blob_tables() {
    let expected = [(64, 18), (82, 18), (0, 64), (100, 18)];
    for (queue, (first, capacity)) in expected.into_iter().enumerate() {
        let reservation =
            MacHeTbLinkReservation::for_queue(MacHeTbTidLimit::One, queue as u8, capacity).unwrap();
        assert_eq!(reservation.first(), first);
        assert_eq!(reservation.count(), capacity);
        assert!(
            MacHeTbLinkReservation::for_queue(MacHeTbTidLimit::One, queue as u8, capacity + 1)
                .is_none()
        );
    }
}

use crate::ieee80211::registers::{HostWords, element};

fn reservation() -> MacHeTbLinkReservation {
    MacHeTbLinkReservation::for_queue(MacHeTbTidLimit::Three, 1, 3).expect("a queue-one range")
}

#[test]
fn a_trigger_queue_links_its_lengths_and_publishes_validity_last() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    let tid = MacHeTid::new(5).expect("a TID");
    let snapshot = owners
        .registers
        .prepare_he_trigger_based_queue(
            MacHeTbTidLimit::Three,
            reservation(),
            tid,
            &[100, 200, 300],
            0x1234,
        )
        .expect("an eligible queue");
    assert_eq!(
        snapshot,
        MacHeTriggerTxQueueSnapshot {
            logical_queue: 1,
            tid: 5,
            trigger_based_enabled: true,
            mu_edca_timer_select: 1,
            mu_edca_timer_enabled: true,
            first_link: 64,
            first_mpdu_length: 100,
            first_next_link: 65,
            tail_link: 66,
            programmed_msdu_bytes: 0x1234,
            queued_msdu_bytes: 0x1234,
            queue_valid: true,
        }
    );
    let registers = &owners.registers;
    let second = registers.he_tx_mpdu_length_link(65).expect("an entry");
    assert_eq!((second.mpdu_length, second.next_link), (200, 66));
    let last = registers.he_tx_mpdu_length_link(66).expect("an entry");
    assert_eq!(
        (last.mpdu_length, last.next_link),
        (
            300,
            oer_esp32s31_pac::wifi_mac::he_scratch::NextLink::End.bits() as u8
        )
    );
    // Logical queue one is CONF1 element six.
    let conf1 = registers
        .mac
        .wifi_mac_he_init_suffix
        .read_queue_control(element(6));
    assert!(conf1.trigger_based_enable());
    assert_eq!(conf1.minimum_tx_power().get(), 0);

    owners.registers.clear_he_trigger_based_queue(reservation());
    assert!(
        !owners
            .registers
            .he_trigger_based_queue_snapshot(reservation())
            .trigger_based_enabled
    );
}

#[test]
fn the_queue_control_store_keeps_only_the_translation_bit() {
    let mut words = HostWords::filled(u32::MAX);
    let mut owners = words.owners();
    owners
        .registers
        .prepare_he_trigger_based_queue(
            MacHeTbTidLimit::Three,
            reservation(),
            MacHeTid::new(0).expect("a TID"),
            &[1, 2, 3],
            0,
        )
        .expect("an eligible queue");
    let conf1 = owners
        .registers
        .mac
        .wifi_mac_he_init_suffix
        .read_queue_control(element(6));
    assert!(conf1.qos_null_to_translated_bss());
    assert_eq!(conf1.low_cleared_by_tb_setup().get(), 0);
    assert_eq!(conf1.high_cleared_by_tb_setup().get(), 0);
    assert_eq!(conf1.minimum_tx_power().get(), 0);
    assert_eq!(conf1.tid().get(), 0);
}

#[test]
fn an_invalid_request_writes_nothing() {
    let before = HostWords::filled(0);
    let mut after = before.clone();
    let mut owners = after.owners();
    let ineligible = MacHeTid::new(1).expect("a TID");
    assert_eq!(
        owners.registers.prepare_he_trigger_based_queue(
            MacHeTbTidLimit::Three,
            reservation(),
            ineligible,
            &[1, 2, 3],
            0,
        ),
        Err(MacHeTbProgramError::TidNotEligible)
    );
    let eligible = MacHeTid::new(0).expect("a TID");
    for (lengths, bytes, error) in [
        (&[1_u16, 2][..], 0, MacHeTbProgramError::LengthCountMismatch),
        (&[1, 0, 3][..], 0, MacHeTbProgramError::InvalidMpduLength),
        (
            &[1, 0x4000, 3][..],
            0,
            MacHeTbProgramError::InvalidMpduLength,
        ),
        (
            &[1, 2, 3][..],
            0x10_0000,
            MacHeTbProgramError::QueuedBytesTooLarge,
        ),
    ] {
        assert_eq!(
            owners.registers.prepare_he_trigger_based_queue(
                MacHeTbTidLimit::Three,
                reservation(),
                eligible,
                lengths,
                bytes,
            ),
            Err(error)
        );
    }
    drop(owners);
    assert!(before.changes(&after).is_empty());
}

#[test]
fn a_tid_bit_and_the_obss_disable_are_published() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners
        .registers
        .set_he_trigger_based_tid_enabled(MacHeTid::new(6).expect("a TID"), true);
    let snapshot = owners.registers.he_buffer_status_snapshot();
    assert_eq!(
        snapshot.trigger_based_tid_enabled,
        [false, false, false, false, false, false, true, false]
    );
    owners.registers.set_he_obss_narrow_band_ru_disabled(true);
    let obss = &owners.registers.mac.wifi_mac_he_obss_narrow_band_ru;
    assert_eq!(obss.read_disable_bitmap().value().get(), 0x000f_ffff);
    let control = obss.read_he_obss_narrow_band_ru_control();
    assert_eq!(control.class_disable().get(), 0x1f);
    assert!(control.global_disable());
}

#[test]
fn the_diagnostic_snapshots_only_read() {
    let before = HostWords::filled(u32::MAX);
    let mut after = before.clone();
    {
        let owners = after.owners();
        let registers = &owners.registers;
        let _ = registers.he_receive_configuration_snapshot();
        let _ = registers.he_queue_scheduling_snapshot();
        let _ = registers.he_beamforming_diagnostics();
        let _ = registers.beamforming_average_snr();
        let diagnostics = registers.he_trigger_receive_diagnostics();
        assert_eq!(
            diagnostics.ru_allocation,
            diagnostics.ru_allocation_raw >> 1
        );
        assert_eq!(diagnostics.cbw20_packet_count, 0x3f);
    }
    assert!(before.changes(&after).is_empty());
}
