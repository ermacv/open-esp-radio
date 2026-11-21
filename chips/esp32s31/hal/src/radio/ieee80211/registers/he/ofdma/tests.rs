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
            let index = reservation.index(position).unwrap().get();
            assert!(!seen[index]);
            seen[index] = true;
            assert_eq!(
                reservation.next(position).unwrap(),
                if position + 1 == capacity {
                    MacHeTbNextLink::End
                } else {
                    MacHeTbNextLink::Link(MacHeTbLink::new(index + 1).unwrap())
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

use crate::radio::ieee80211::registers::{HostWords, element};

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
            &lengths([100, 200, 300]),
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
            oer_chips_esp32s31_pac::wifi_mac::he_scratch::NextLink::End.bits() as u8
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
            &lengths([1, 2, 3]),
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
            &lengths([1, 2, 3]),
            0,
        ),
        Err(MacHeTbProgramError::TidNotEligible)
    );
    let eligible = MacHeTid::new(0).expect("a TID");
    let two = lengths([1, 2]);
    let three = lengths([1, 2, 3]);
    for (lengths, bytes, error) in [
        (&two[..], 0, MacHeTbProgramError::LengthCountMismatch),
        (
            &three[..],
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
    let snapshot = he_buffer_status_snapshot(&owners.registers);
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

// Reads of what the writers left, from the register image: the HAL keeps
// no read of what it wrote.

/// One non-latched view of all hardware-visible HE Buffer Status Reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MacHeBufferStatusSnapshot {
    hardware: [u32; MacHeTid::COUNT],
    software: [u32; MacHeTid::COUNT],
    valid: [bool; MacHeTid::COUNT],
    trigger_based_tid_enabled: [bool; MacHeTid::COUNT],
    ac_empty_software_tid: u8,
    ac_empty_uses_software_tid: bool,
    basic_special_bsr_sequence: bool,
    tid_limit_zero_bsr_sequence: bool,
}

/// Allocation-free view of the HE/RX policy words decoded by
/// `dbg_read_rx_misc`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MacHeReceiveConfigurationSnapshot {
    bss_color: u8,
    bss_color_enabled: bool,
    partial_bss_color_enabled: bool,
    bssid_select: u8,
    he_bssid_enabled: bool,
    multi_bssid_enabled: bool,
    multi_bssid_mask: u8,
    co_hosted_enabled: bool,
    default_packet_extension_duration: u8,
    mpdu_length_offset: u16,
    nfrp_buffer_threshold: u8,
    hardware_txop_enabled: bool,
    bsr_update_enabled: bool,
    trigger_based_stop_option: bool,
    trigger_response_scheduling_supported: bool,
    uplink_mu_data_disabled: bool,
    uplink_mu_disabled: bool,
    trigger_based_no_resource_continue_tx: bool,
    automatic_ack_allows_extended_range_su: bool,
    he_response_ack: bool,
    nominal_packet_padding_duration: [u8; 5],
    rx_control_9_bssid_position: u8,
    power_save: MacHeRxPowerSaveSnapshot,
    /// Profiles are ordered exactly as CUSTOM_TYPE0, 1 and 2.
    custom_receive_types: [MacHeCustomReceiveType; 3],
    beamforming: MacHeBeamformingConfigurationSnapshot,
}

/// Receive power-save fields decoded by `dbg_read_rx_misc`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MacHeRxPowerSaveSnapshot {
    threshold: u16,
    enabled: bool,
    stop_rf: bool,
    ready: bool,
    front_end_frequency_hop_time: u8,
    phy_signal_delay: u8,
    intra_bss_color_check_enabled: bool,
    intra_ppdu_enabled: bool,
    vht_txop_address_check_enabled: bool,
    vht_txop_enabled: bool,
}

/// One custom receive frame-type matcher and its automatic-ACK selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MacHeCustomReceiveType {
    enabled: bool,
    value: u8,
    ack_index: u8,
}

/// Receive beamforming state decoded from `WDEVBEAMFORMCONF`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MacHeBeamformingConfigurationSnapshot {
    memory_write_enabled: bool,
    bfrp_time: u8,
    ndp_time: u8,
    hardware_sequence_select: u8,
    hardware_sequence_enabled: bool,
    he_beam_enabled: bool,
    non_trigger_based_ru_select: bool,
    ru_select: bool,
}

/// Fixed-allocation snapshot of all recovered HE queue scheduling fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MacHeQueueSchedulingSnapshot {
    trigger_queues: [MacHeTriggerQueueConfiguration; 8],
    edca_queues: [MacHeEdcaQueueConfiguration; 4],
    mu_edca_timers: [MacHeMuEdcaTimerSnapshot; 4],
}

/// WDEVTXQ_CONF1 fields for one of the eight reverse-addressed logical queues.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MacHeTriggerQueueConfiguration {
    tid: u8,
    trigger_based_enabled: bool,
    mu_edca_timer_select: u8,
    mpdu_length_link_address: u8,
    minimum_tx_power: u8,
}

/// WDEVTXQ_CONF2 fields for one of the four ordinary EDCA queues.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MacHeEdcaQueueConfiguration {
    minimum_mpdu_length_cbw20: u16,
    minimum_mpdu_length_cbw40: u16,
    minimum_mpdu_length_cbw80: u16,
    software_rts: bool,
    software_cts: bool,
}

/// One MU-EDCA timer decoded in the blob's eight-TU units.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MacHeMuEdcaTimerSnapshot {
    timer_8tu: u8,
    enabled: bool,
    reset: bool,
    current_count_8tu: u8,
    reached: bool,
    aifs: u8,
}

/// Read all eight interleaved hardware/software BSR values and control.
///
/// SOURCE: complete pinned `libpp.a[hal_debug.o]`
/// `dbg_read_bsr_info` and `libpp.a[hal_mac_ctl.o]`
/// `hal_he_get_hw_txq_bsr`.
fn he_buffer_status_snapshot<M: MacPlacement>(
    registers: &WifiRadioRegisters<M>,
) -> MacHeBufferStatusSnapshot {
    let registers = &registers.mac.wifi_mac_he_buffer_status;
    let mut hardware = [0_u32; MacHeTid::COUNT];
    let mut software = [0_u32; MacHeTid::COUNT];
    for tid in 0..MacHeTid::COUNT {
        hardware[tid] = registers.read_hardware_bsr(element(tid)).value().get();
        software[tid] = registers.read_software_bsr(element(tid)).value().get();
    }
    let control = registers.read_he_buffer_status_control();

    MacHeBufferStatusSnapshot {
        hardware,
        software,
        valid: [
            control.valid_0(),
            control.valid_1(),
            control.valid_2(),
            control.valid_3(),
            control.valid_4(),
            control.valid_5(),
            control.valid_6(),
            control.valid_7(),
        ],
        trigger_based_tid_enabled: [
            control.trigger_based_tid_0_enabled(),
            control.trigger_based_tid_1_enabled(),
            control.trigger_based_tid_2_enabled(),
            control.trigger_based_tid_3_enabled(),
            control.trigger_based_tid_4_enabled(),
            control.trigger_based_tid_5_enabled(),
            control.trigger_based_tid_6_enabled(),
            control.trigger_based_tid_7_enabled(),
        ],
        ac_empty_software_tid: byte(control.ac_empty_software_tid()),
        ac_empty_uses_software_tid: control.ac_empty_use_software_tid(),
        basic_special_bsr_sequence: control.basic_special_bsr_sequence(),
        tid_limit_zero_bsr_sequence: control.tid_limit_zero_bsr_sequence(),
    }
}

/// Read all HE/RX configuration fields recovered from `dbg_read_rx_misc`.
///
/// This is a best-effort multi-register sample. It deliberately exposes
/// the blob's hardware polarity: `uplink_mu_disabled == true` means the
/// disable bit is set, while `hardware_txop_enabled == true` means the
/// blob's `HE_USE_SW_TXOP` bit is clear.
///
/// SOURCE: complete pinned `libpp.a[hal_debug.o]`
/// `dbg_read_rx_misc`, size `0x42a`, and its function-local strings.
fn he_receive_configuration_snapshot<M: MacPlacement>(
    registers: &WifiRadioRegisters<M>,
) -> MacHeReceiveConfigurationSnapshot {
    let suffix = &registers.mac.wifi_mac_he_init_suffix;
    let multi = suffix.read_multi_bssid_control();
    let options1 = suffix.read_ersu_and_vht_control();
    let options2 = suffix.read_he_default_control();
    let padding = suffix.read_he_packet_padding();
    let power_save = registers
        .mac
        .wifi_mac_rx_power_save
        .read_rx_power_save_control();
    let custom = registers
        .mac
        .wifi_mac_rx_custom_type
        .read_rx_custom_type_control();
    let beamforming = registers
        .mac
        .wifi_mac_he_init_prefix
        .read_bf_timing_control();

    MacHeReceiveConfigurationSnapshot {
        bss_color: byte(multi.bss_color()),
        bss_color_enabled: multi.bss_color_enable(),
        partial_bss_color_enabled: multi.partial_bss_color_enable(),
        bssid_select: byte(multi.bssid_select()),
        he_bssid_enabled: multi.he_bssid_enable(),
        multi_bssid_enabled: multi.multi_bssid_enable(),
        multi_bssid_mask: multi.multi_bssid_mask(),
        co_hosted_enabled: multi.co_hosted_enable(),
        default_packet_extension_duration: byte(options2.default_pe_duration()),
        mpdu_length_offset: halfword(options2.mpdu_length_offset()),
        nfrp_buffer_threshold: options2.nfrp_buffer_threshold(),
        hardware_txop_enabled: !options2.he_use_software_txop(),
        bsr_update_enabled: options2.bsr_update_enable(),
        trigger_based_stop_option: options2.tb_stop_option(),
        trigger_response_scheduling_supported: options2.he_trs_support(),
        uplink_mu_data_disabled: options2.ul_mu_data_disable(),
        uplink_mu_disabled: options2.ul_mu_disable(),
        trigger_based_no_resource_continue_tx: options1.tb_no_resource_continue_tx(),
        automatic_ack_allows_extended_range_su: options1.auto_ack_allow_ersu(),
        he_response_ack: options1.he_response_ack(),
        nominal_packet_padding_duration: [
            byte(padding.bpsk_duration()),
            byte(padding.qpsk_duration()),
            byte(padding.qam16_duration()),
            byte(padding.qam64_duration()),
            byte(padding.qam256_duration()),
        ],
        rx_control_9_bssid_position: byte(
            registers
                .mac
                .wifi_mac_rx_bssid_list
                .read_rx_bssid_list_control()
                .rx_control_9_bssid_position(),
        ),
        power_save: MacHeRxPowerSaveSnapshot {
            threshold: halfword(power_save.rx_ps_threshold()),
            enabled: power_save.rx_ps_enable(),
            stop_rf: power_save.stop_rf(),
            ready: power_save.ps_ready(),
            front_end_frequency_hop_time: power_save.fe_frequency_hop_time(),
            phy_signal_delay: byte(power_save.rx_phy_signal_delay()),
            intra_bss_color_check_enabled: power_save.intra_ps_check_bss_color_enable(),
            intra_ppdu_enabled: power_save.intra_ppdu_ps_enable(),
            vht_txop_address_check_enabled: power_save.rx_vht_txop_ps_check_address(),
            vht_txop_enabled: power_save.rx_vht_txop_ps_enable(),
        },
        custom_receive_types: [
            MacHeCustomReceiveType {
                enabled: custom.type_0_enable(),
                value: byte(custom.type_0_value()),
                ack_index: byte(custom.type_0_ack_index()),
            },
            MacHeCustomReceiveType {
                enabled: custom.type_1_enable(),
                value: byte(custom.type_1_value()),
                ack_index: byte(custom.type_1_ack_index()),
            },
            MacHeCustomReceiveType {
                enabled: custom.type_2_enable(),
                value: byte(custom.type_2_value()),
                ack_index: byte(custom.type_2_ack_index()),
            },
        ],
        beamforming: MacHeBeamformingConfigurationSnapshot {
            memory_write_enabled: beamforming.bf_memory_write_enable(),
            bfrp_time: byte(beamforming.rx_bfrp_timeout_ms()),
            ndp_time: beamforming.he_beam_ndp_time(),
            hardware_sequence_select: byte(beamforming.he_beam_hw_sequence_select()),
            hardware_sequence_enabled: beamforming.he_beam_hw_sequence_enable(),
            he_beam_enabled: beamforming.he_beam_enable(),
            non_trigger_based_ru_select: beamforming.non_tb_beam_ru_select(),
            ru_select: beamforming.beam_ru_select(),
        },
    }
}

/// Read queue-local TB, TID, minimum-MPDU and MU-EDCA state.
///
/// SOURCE: complete pinned `libpp.a[hal_debug.o]`
/// `dbg_read_txq_conf1`, `dbg_read_txq_conf2` and
/// `dbg_read_muedca_timer`. CONF1 and CONF2 use reverse logical queue
/// addressing, normalized here so array index equals the blob's logged
/// logical queue number.
fn he_queue_scheduling_snapshot<M: MacPlacement>(
    registers: &WifiRadioRegisters<M>,
) -> MacHeQueueSchedulingSnapshot {
    let suffix = &registers.mac.wifi_mac_he_init_suffix;
    let queue_control = &registers.mac.wifi_mac_tx_queue_control;
    let timers = &registers.mac.wifi_mac_he_mu_edca_timer;

    let trigger_queues = core::array::from_fn(|logical_queue| {
        let queue = suffix.read_queue_control(element(7 - logical_queue));
        MacHeTriggerQueueConfiguration {
            tid: byte(queue.tid()),
            trigger_based_enabled: queue.trigger_based_enable(),
            mu_edca_timer_select: byte(queue.mu_edca_timer_select()),
            mpdu_length_link_address: byte(queue.mpdu_length_link_address()),
            minimum_tx_power: byte(queue.minimum_tx_power()),
        }
    });
    let edca_queues = core::array::from_fn(|logical_queue| {
        let queue = queue_control.read_protection(element(3 - logical_queue));
        MacHeEdcaQueueConfiguration {
            minimum_mpdu_length_cbw20: halfword(queue.minimum_mpdu_length_cbw20()),
            minimum_mpdu_length_cbw40: halfword(queue.minimum_mpdu_length_cbw40()),
            minimum_mpdu_length_cbw80: halfword(queue.minimum_mpdu_length_cbw80()),
            software_rts: matches!(
                queue.software_rts(),
                oer_chips_esp32s31_pac::wifi_mac::protection::SoftwareRts::Requested
            ),
            software_cts: matches!(
                queue.software_cts(),
                oer_chips_esp32s31_pac::wifi_mac::protection::SoftwareCts::Requested
            ),
        }
    });
    let mu_edca_timers = core::array::from_fn(|index| {
        let timer = timers.read_timer(element(index));
        MacHeMuEdcaTimerSnapshot {
            timer_8tu: timer.timer_8tu(),
            enabled: timer.enable(),
            reset: timer.reset(),
            current_count_8tu: timer.current_count_8tu(),
            reached: timer.reached(),
            aifs: byte(timer.aifs()),
        }
    });

    MacHeQueueSchedulingSnapshot {
        trigger_queues,
        edca_queues,
        mu_edca_timers,
    }
}

fn lengths<const N: usize>(values: [u16; N]) -> [MacHeMpduLength; N] {
    values.map(|value| MacHeMpduLength::new(value).expect("a link entry's length"))
}

#[test]
fn an_mpdu_length_is_non_zero_and_fits_its_fourteen_bits() {
    assert_eq!(MacHeMpduLength::new(0), None);
    assert_eq!(MacHeMpduLength::new(0x4000), None);
    assert_eq!(
        MacHeMpduLength::new(0x3fff).map(|length| length.field().get()),
        Some(0x3fff)
    );
    assert_eq!(
        MacHeMpduLength::new(1).map(|length| length.field().get()),
        Some(1)
    );
}

#[test]
fn a_tid_holds_the_eight_values_of_its_field() {
    for value in 0..8 {
        assert_eq!(MacHeTid::new(value).map(MacHeTid::value), Some(value));
    }
    assert_eq!(MacHeTid::new(8), None);
}
