//! The MAC owners' register operations over host memory: what each writes,
//! what it keeps, and the ownership of the interrupt side.

use std::{vec, vec::Vec};

use oer_esp32s31_pac::{
    ieee802154::command::Opcode,
    modem_etm::{ch_event, ch_task},
};
use oer_espressif_ieee802154_engine::types::{
    Ieee802154CcaMode, Ieee802154DebugCounter, Ieee802154EdSampleMode, Ieee802154EtmChannel,
    Ieee802154EtmRoute, Ieee802154Event, Ieee802154EventMask, Ieee802154MultipanEnableState,
    Ieee802154MultipanIndex, Ieee802154RxAbortEnableSet, Ieee802154RxAbortReasonObservation,
    Ieee802154TxAbortEnableSet,
};

use super::{
    Ieee802154Pti,
    host::{HostMemory, changed},
};
use crate::ieee802154::{
    lifecycle::Ieee802154Channel,
    policy::{
        Ieee802154AckTimeout, Ieee802154MacControl, Ieee802154MacPolicySnapshot,
        Ieee802154PanIdentity,
    },
};

fn mask(events: &[Ieee802154Event]) -> Ieee802154EventMask {
    events
        .iter()
        .fold(Ieee802154EventMask::NONE, |mask, event| mask.with(*event))
}

#[test]
fn assembling_host_owners_performs_no_register_access() {
    let mut memory = HostMemory::filled(0xa5a5_a5a5);
    let before = (
        memory.mac.clone(),
        memory.etm.clone(),
        memory.baseband.clone(),
    );
    let _ = memory.owners();
    assert_eq!((memory.mac, memory.etm, memory.baseband), before);
}

#[test]
fn arming_masks_the_events_and_the_session_enables_then_masks_them() {
    let mut memory = HostMemory::filled(u32::MAX);
    let (task, setup) = memory.owners();
    let (startup, interrupts) = setup.arm(task);
    let mut task = startup.task;
    // Arming leaves every event masked: nothing is raised before a handler
    // holds the owner (`enable_events` takes the binding's proof).
    assert!(task.foundation_snapshot().events_masked());

    task.publish_runtime_events();
    let events = task.mac_part().read_event_enable();
    for (enabled, served) in [
        (events.tx_done(), true),
        (events.rx_done(), true),
        (events.ack_tx_done(), true),
        (events.ack_rx_done(), true),
        (events.rx_abort(), true),
        (events.tx_abort(), true),
        (events.ed_done(), true),
        (events.events_7(), false),
        (events.timer0_overflow(), false),
        (events.timer1_overflow(), true),
        (events.clock_count_match(), false),
        (events.tx_sfd_done(), true),
        (events.rx_sfd_done(), true),
        (events.events_13(), false),
    ] {
        assert_eq!(enabled, served);
    }
    let rx = task.mac_part().read_rx_abort_enable();
    assert!(rx.tx_ack_timeout() && rx.tx_ack_coexistence_break());
    assert!(!(rx.rx_stop() || rx.crc_error() || rx.tx_ack_stop() || rx.ed_abort()));
    assert_eq!(rx.rx_abort_en_9_14().get(), 0);
    let tx = task.mac_part().read_tx_abort_enable();
    assert!(
        tx.rx_ack_timeout()
            && tx.tx_coexistence_break()
            && tx.tx_security_error()
            && tx.cca_failed()
            && tx.cca_busy()
    );
    assert!(!(tx.rx_ack_stop() || tx.tx_stop() || tx.rx_ack_crc_error()));
    assert_eq!(tx.tx_abort_en_25_30().get(), 0);
    // Arming leaves the status image untouched.
    assert!(
        interrupts
            .events()
            .observation()
            .contains(Ieee802154Event::TxDone)
    );

    // The session's end masks every event and keeps the aborts.
    task.mask_events();
    let foundation = task.foundation_snapshot();
    assert!(foundation.events_masked());
    assert!(!foundation.rx_aborts_masked());

    let setup = interrupts.deactivate(&mut task);
    let foundation = task.foundation_snapshot();
    assert!(foundation.events_masked());
    assert!(foundation.rx_aborts_masked());
    assert!(foundation.tx_aborts_masked());
    assert!(!setup.state_snapshot().all_codes_zero());
}

#[test]
fn clearing_events_acknowledges_only_the_sampled_events_the_mask_selects() {
    let mut memory = HostMemory::filled(u32::MAX);
    let (task, setup) = memory.owners();
    let (startup, mut interrupts) = setup.arm(task);
    let mut task = startup.task;
    // Arming leaves the status image untouched, including unnamed lanes.
    assert!(interrupts.events().observation().classification().is_err());

    let selected = mask(&[Ieee802154Event::TxDone, Ieee802154Event::RxSfdDone]);
    let snapshot = interrupts.events();
    interrupts.clear_events(snapshot, selected);
    // Host memory keeps what the acknowledgement wrote: the two selected
    // events and no unnamed one.
    assert_eq!(
        interrupts.events().observation().classification(),
        Ok(selected)
    );

    let snapshot = interrupts.events();
    interrupts.clear_events(snapshot, Ieee802154EventMask::NONE);
    assert!(interrupts.events().observation().is_clear());
    let _setup = interrupts.deactivate(&mut task);
}

#[test]
fn an_abort_set_changes_only_its_own_fields() {
    let mut memory = HostMemory::filled(0);
    let (mut task, _setup) = memory.owners();

    task.set_rx_aborts(Ieee802154RxAbortEnableSet::RuntimeBaseline, true);
    let rx = task.mac_part().read_rx_abort_enable();
    assert!(rx.tx_ack_timeout() && rx.tx_ack_coexistence_break());
    assert!(!rx.rx_stop() && !rx.tx_ack_stop());

    task.set_rx_aborts(Ieee802154RxAbortEnableSet::All, true);
    task.set_rx_aborts(Ieee802154RxAbortEnableSet::RuntimeBaseline, false);
    let rx = task.mac_part().read_rx_abort_enable();
    assert!(!rx.tx_ack_timeout() && !rx.tx_ack_coexistence_break());
    assert!(rx.rx_stop() && rx.tx_ack_stop() && rx.ed_coexistence_reject());
    assert_eq!(rx.rx_abort_en_26_30().get(), 0x1f);

    task.set_tx_aborts(Ieee802154TxAbortEnableSet::All, true);
    task.set_tx_aborts(Ieee802154TxAbortEnableSet::RuntimeBaseline, false);
    let tx = task.mac_part().read_tx_abort_enable();
    assert!(!tx.rx_ack_timeout() && !tx.cca_busy());
    assert!(tx.rx_ack_stop() && tx.tx_stop());
    assert_eq!(tx.tx_abort_en_9_14().get(), 0x3f);

    task.enable_only_runtime_rx_aborts();
    task.enable_only_runtime_tx_aborts();
    let rx = task.mac_part().read_rx_abort_enable();
    let tx = task.mac_part().read_tx_abort_enable();
    assert!(rx.tx_ack_timeout() && !rx.rx_stop());
    assert!(tx.cca_busy() && !tx.tx_stop());

    task.mask_all_rx_aborts();
    task.mask_all_tx_aborts();
    let foundation = task.foundation_snapshot();
    assert!(foundation.rx_aborts_masked() && foundation.tx_aborts_masked());
}

#[test]
fn the_foundation_writes_read_back_as_the_foundation() {
    let mut memory = HostMemory::filled(u32::MAX);
    let (mut task, _setup) = memory.owners();
    let before = task.foundation_snapshot();
    assert!(!before.events_masked() && !before.txon_delay_applied());

    task.mask_all_events();
    task.mask_all_rx_aborts();
    task.mask_all_tx_aborts();
    task.set_ed_sample_mode(Ieee802154EdSampleMode::Average);
    task.set_txrx_pti(Ieee802154Pti::COEX_DISABLED);
    task.set_ack_pti(Ieee802154Pti::COEX_DISABLED);
    task.apply_txon_delay();

    let foundation = task.foundation_snapshot();
    assert!(foundation.events_masked());
    assert!(foundation.rx_aborts_masked());
    assert!(foundation.tx_aborts_masked());
    assert!(foundation.ed_uses_average());
    assert_eq!(foundation.txrx_pti(), Ieee802154Pti::COEX_DISABLED);
    assert_eq!(foundation.ack_pti(), Ieee802154Pti::COEX_DISABLED);
    assert!(foundation.txon_delay_applied());
}

#[test]
fn the_vendor_delays_change_four_fields_and_keep_every_other_bit() {
    let before = HostMemory::filled(u32::MAX);
    let mut memory = HostMemory::filled(u32::MAX);
    {
        let (mut task, _setup) = memory.owners();
        task.apply_txon_delay();
        let sequence = task.power_sequence();
        assert_eq!(sequence.tx_on_delay(), 45);
        assert_eq!(sequence.rx_on_delay(), 50);
        assert_eq!(sequence.txrx_switch_delay(), 117);
        assert_eq!(sequence.tx_off_delay(), 5);
        assert_eq!(
            sequence.pa_on_delay(),
            0x3ff,
            "an unwritten delay keeps its value"
        );
    }
    assert_eq!(
        changed(&before.mac, &memory.mac).len(),
        4,
        "one word per delay"
    );
}

#[test]
fn the_static_policy_reads_back_through_the_policy_snapshot() {
    let mut memory = HostMemory::filled(0);
    let (mut task, _setup) = memory.owners();
    let channel = Ieee802154Channel::new(20).expect("standard channel");
    let control = Ieee802154MacControl::new(true, false, true, false, true, false);
    let timeout = Ieee802154AckTimeout::from_microseconds(1_729).expect("bounded timeout");
    let identity = Ieee802154PanIdentity::new(
        0x1a2b,
        0x3c4d,
        [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87],
    );

    task.set_channel(channel);
    task.set_cca_mode(Ieee802154CcaMode::CarrierOrEnergyDetection);
    task.set_cca_threshold_code(-67);
    task.set_mac_control(control);
    task.set_ack_timeout(timeout.units());
    task.set_primary_pan_identity(identity);

    assert_eq!(
        task.mac_policy_snapshot(),
        Ieee802154MacPolicySnapshot::new(
            channel.frequency_code(),
            Ieee802154CcaMode::CarrierOrEnergyDetection,
            -67,
            timeout,
            control,
            Ieee802154MultipanEnableState::new(true, false, false, false),
            identity,
        )
    );
}

#[test]
fn an_address_setter_enables_its_context_and_reaches_only_it() {
    let mut memory = HostMemory::filled(0);
    let (mut task, _setup) = memory.owners();
    let third = Ieee802154MultipanIndex::CONTEXT2;
    let address = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];

    task.set_multipan_short_address(third, 0xbeef);
    assert_eq!(
        task.multipan_enable(),
        Ieee802154MultipanEnableState::new(false, false, true, false)
    );
    task.set_multipan_pan_id(third, 0x1234);
    task.set_multipan_extended_address(third, address);
    assert_eq!(task.multipan_short_address(third), 0xbeef);
    assert_eq!(task.multipan_pan_id(third), 0x1234);
    assert_eq!(task.multipan_extended_address(third), address);
    let first = Ieee802154MultipanIndex::CONTEXT0;
    assert_eq!(task.multipan_short_address(first), 0);
    assert_eq!(task.multipan_extended_address(first), [0; 8]);

    // The enable state is replaced whole and keeps the identities.
    task.set_multipan_enable(Ieee802154MultipanEnableState::NONE);
    assert_eq!(task.multipan_enable(), Ieee802154MultipanEnableState::NONE);
    assert_eq!(task.multipan_short_address(third), 0xbeef);
}

#[test]
fn the_security_key_and_address_are_written_as_little_endian_words() {
    let before = HostMemory::filled(0);
    let mut memory = HostMemory::filled(0);
    {
        let (mut task, _setup) = memory.owners();
        task.set_security_key(&[
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ]);
        task.set_security_address(&[0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe]);
    }
    let mut words = changed(&before.mac, &memory.mac);
    words.sort_unstable();
    let mut expected = vec![
        0x3322_1100,
        0x7766_5544,
        0xbbaa_9988,
        0xffee_ddcc,
        0x7654_3210,
        0xfedc_ba98,
    ];
    expected.sort_unstable();
    assert_eq!(words, expected);
}

#[test]
fn the_payload_offset_keeps_seven_bits_and_the_enable() {
    let mut memory = HostMemory::filled(0);
    let (mut task, _setup) = memory.owners();
    task.set_transmit_security(true);
    task.set_security_offset(0xff);
    let control = task.mac_part().read_security_control();
    assert_eq!(control.payload_offset().get(), 0x7f);
    assert!(control.tx_enable());
    task.set_transmit_security(false);
    let control = task.mac_part().read_security_control();
    assert_eq!(control.payload_offset().get(), 0x7f);
    assert!(!control.tx_enable());
}

#[test]
fn the_energy_detection_duration_keeps_the_delay_field() {
    let mut memory = HostMemory::filled(u32::MAX);
    let (mut task, _setup) = memory.owners();
    task.set_ed_duration(0x1234);
    let duration = task.mac_part().read_ed_duration();
    assert_eq!(duration.duration().get(), 0x1234);
    assert_eq!(duration.delay().get(), 0xf);
}

#[test]
fn a_command_writes_its_one_image_from_zero() {
    for opcode in [
        Opcode::TxStart,
        Opcode::RxStart,
        Opcode::CcaTxStart,
        Opcode::EdStart,
        Opcode::Stop,
        Opcode::Timer0Start,
        Opcode::Timer0Stop,
        Opcode::Timer1Start,
        Opcode::Timer1Stop,
    ] {
        let before = HostMemory::filled(u32::MAX);
        let mut memory = HostMemory::filled(u32::MAX);
        {
            let (mut task, _setup) = memory.owners();
            task.issue(opcode);
        }
        assert_eq!(changed(&before.mac, &memory.mac), [opcode.bits()]);
    }
}

#[test]
fn a_counter_clear_writes_one_field_from_zero() {
    let before = HostMemory::filled(u32::MAX);
    let mut memory = HostMemory::filled(u32::MAX);
    {
        let (mut task, _setup) = memory.owners();
        task.clear_debug_counter(Ieee802154DebugCounter::CrcError);
    }
    let words = changed(&before.mac, &memory.mac);
    assert_eq!(words.len(), 1, "one word written");
    assert_eq!(words[0].count_ones(), 1, "one field set, every other zero");
}

#[test]
fn an_unnamed_abort_code_is_unclassified() {
    let mut memory = HostMemory::filled(u32::MAX);
    let (task, _setup) = memory.owners();
    let status = task.rx_status();
    assert_eq!(
        status.abort_reason(),
        Ieee802154RxAbortReasonObservation::Unclassified
    );
    assert!(status.frame_in_progress());
}

#[test]
fn the_etm_routes_program_their_own_channel() {
    let mut memory = HostMemory::filled(0);
    let (mut task, _setup) = memory.owners();
    task.set_etm_route(Ieee802154EtmRoute::Timer0ToTxStart);
    task.set_etm_route(Ieee802154EtmRoute::Timer1ToRxStart);
    let etm = task.etm_part();
    assert_eq!(
        etm.read_ch0_event().id_variant(),
        Some(ch_event::Id::Ieee802154Timer0Overflow)
    );
    assert_eq!(
        etm.read_ch0_task().id_variant(),
        Some(ch_task::Id::Ieee802154TxStart)
    );
    assert_eq!(
        etm.read_ch1_event().id_variant(),
        Some(ch_event::Id::Ieee802154Timer1Overflow)
    );
    assert_eq!(
        etm.read_ch1_task().id_variant(),
        Some(ch_task::Id::Ieee802154RxStart)
    );

    task.set_etm_route(Ieee802154EtmRoute::Timer0ToCcaTx);
    assert_eq!(
        task.etm_part().read_ch0_task().id_variant(),
        Some(ch_task::Id::Ieee802154EdTrigTx)
    );
}

#[test]
fn a_channel_strobe_writes_one_trigger_word_and_reads_the_enable_word() {
    let before = HostMemory::filled(0);
    let mut memory = HostMemory::filled(0);
    {
        let (mut task, _setup) = memory.owners();
        task.enable_etm_channel(Ieee802154EtmChannel::Channel0);
        task.disable_etm_channel(Ieee802154EtmChannel::Channel1);
        // The strobes write the set and clear words, not the enable word.
        assert!(!task.etm_channel_enabled(Ieee802154EtmChannel::Channel0));
    }
    let words: Vec<u32> = changed(&before.etm, &memory.etm);
    assert_eq!(words.len(), 2, "one set and one clear word");
    assert!(words.iter().all(|word| word.count_ones() == 1));

    let mut enabled = HostMemory::filled(u32::MAX);
    let (task, _setup) = enabled.owners();
    assert!(task.etm_channel_enabled(Ieee802154EtmChannel::Channel1));
}

#[test]
fn the_recent_rssi_is_the_signed_low_byte_of_the_receive_information() {
    let mut memory = HostMemory::filled(0);
    memory.baseband.fill(0xffff_ff9c);
    let (task, _setup) = memory.owners();
    assert_eq!(task.recent_rssi(), -100);
}

#[test]
fn a_saved_timer_sample_is_acknowledged_after_masking_without_resampling() {
    let mut memory = HostMemory::filled(u32::MAX);
    let (task, setup) = memory.owners();
    let (startup, mut interrupts) = setup.arm(task);
    let mut task = startup.task;
    let saved = interrupts.events();
    // Host memory stores the acknowledgement image. Make the current status
    // zero to model the hidden read under mask, retaining the earlier sample.
    let other = interrupts.events();
    interrupts.clear_events(other, Ieee802154EventMask::NONE);
    task.mask_events();
    assert!(interrupts.events().observation().is_clear());

    let timers = mask(&[
        Ieee802154Event::Timer0Overflow,
        Ieee802154Event::Timer1Overflow,
    ]);
    interrupts.clear_events(saved, timers);
    assert_eq!(
        interrupts.events().observation().classification(),
        Ok(timers)
    );
    let _setup = interrupts.deactivate(&mut task);
}

#[test]
fn startup_exposes_only_handled_events_and_keeps_runtime_selection_unchanged() {
    let mut memory = HostMemory::filled(0);
    let (task, setup) = memory.owners();
    let (mut startup, _interrupts) = setup.arm(task);
    assert!(startup.task.foundation_snapshot().events_masked());
    startup.task.enable_handled_events();
    let events = startup.task.mac_part().read_event_enable();
    assert!(events.tx_done() && events.rx_done() && events.ack_tx_done() && events.ack_rx_done());
    assert!(events.rx_abort() && events.tx_abort() && events.ed_done());
    assert!(events.timer0_overflow() && events.timer1_overflow());
    assert!(events.rx_sfd_done() && events.tx_sfd_done());
    assert!(!events.events_7() && !events.clock_count_match() && !events.events_13());
    startup.task.publish_runtime_events();
    let events = startup.task.mac_part().read_event_enable();
    assert!(!events.timer0_overflow() && events.timer1_overflow());
    assert!(!events.events_7() && !events.clock_count_match() && !events.events_13());
}

#[test]
fn unsupported_startup_observations_quarantine_both_polled_owners() {
    let mut memory = HostMemory::filled(u32::MAX);
    let (task, setup) = memory.owners();
    let (startup, interrupts) = setup.arm(task);
    let observed = interrupts.events().observation();
    let quarantine = match crate::ieee802154::ll::Ieee802154MacOwners::new(
        startup,
        interrupts,
        super::host::silent(),
    ) {
        Ok(_) => panic!("unsupported startup cannot release operational owners"),
        Err(quarantine) => quarantine,
    };
    assert_eq!(
        quarantine.fault(),
        super::Fault::StartupEvents {
            observation: observed
        }
    );
    let (startup, interrupts, _) = &*quarantine._owners;
    assert!(startup.task.foundation_snapshot().events_masked());
    assert_eq!(
        interrupts.events().observation(),
        observed,
        "fault does not acknowledge any source lane"
    );
}
