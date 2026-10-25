use super::*;
use crate::ieee80211::registers::HostWords;

#[test]
fn logical_queues_reverse_the_four_physical_banks() {
    assert_eq!(physical_bank(0), 3);
    assert_eq!(physical_bank(1), 2);
    assert_eq!(physical_bank(2), 1);
    assert_eq!(physical_bank(3), 0);
    assert_eq!(he_queue_control(0).get(), 7);
    assert_eq!(he_queue_control(3).get(), 4);
}

#[test]
fn completion_acknowledge_writes_the_sample_and_the_selected_queue() {
    let mut memory = HostWords::filled(0);
    let before = memory.clone();
    {
        let mut owners = memory.owners();
        acknowledge_completion(&mut owners.registers.mac.wifi_mac_tx_common, 2);
    }
    // A clear word that read zero is written back with queue two's event
    // alone, and nothing else changes.
    let changes = before.changes(&memory);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].0, "wifi_mac_tx_common");
    assert_eq!(
        changes[0].2,
        AcknowledgeMacTxCompletionConsumed::Queue2.bits()
    );
}

#[test]
fn completion_acknowledge_keeps_every_sampled_event() {
    // The clear word's place, from an acknowledge over zeroed memory.
    let zero = HostWords::filled(0);
    let mut probe = zero.clone();
    acknowledge_completion(&mut probe.owners().registers.mac.wifi_mac_tx_common, 0);
    let (_, offset, _) = zero.changes(&probe)[0];

    let pattern = 0x8000_0001;
    let mut before = HostWords::filled(0);
    before.wifi_mac_tx_common[offset / 4] = pattern;
    let mut memory = before.clone();
    acknowledge_completion(&mut memory.owners().registers.mac.wifi_mac_tx_common, 3);
    let changes = before.changes(&memory);
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0].2,
        pattern | AcknowledgeMacTxCompletionConsumed::Queue3.bits()
    );
}

#[test]
fn collision_and_timeout_clears_strobe_one_queue_each() {
    for queue in 0..ORDINARY_QUEUE_COUNT {
        let before = HostWords::filled(0);
        let mut memory = before.clone();
        {
            let mut owners = memory.owners();
            acknowledge_collision(&mut owners.registers.mac.wifi_mac_tx_common, queue);
        }
        let collision = before.changes(&memory);
        assert_eq!(collision.len(), 1);
        assert_eq!(collision[0].2, 1 << queue);

        let mut memory = before.clone();
        {
            let mut owners = memory.owners();
            acknowledge_timeout(&mut owners.registers.mac.wifi_mac_tx_common, queue);
        }
        let timeout = before.changes(&memory);
        assert_eq!(timeout.len(), 1);
        assert_eq!(timeout[0].1, collision[0].1);
        assert_eq!(timeout[0].2, 1 << (16 + queue));
    }
}

#[test]
fn edca_publishes_three_fields_of_the_reversed_bank() {
    let mut memory = HostWords::filled(0);
    {
        let mut owners = memory.owners();
        configure_edca(
            &mut owners.registers.mac.wifi_mac_tx_queue_control,
            1,
            0x7,
            0x3ff,
            MacInterface::AccessPoint,
        );
    }
    let owners = memory.owners();
    let control = &owners.registers.mac.wifi_mac_tx_queue_control;
    let config = control.read_config(bank(1));
    assert_eq!(config.aifsn().get(), 0x7);
    assert_eq!(config.contention_window().get(), 0x3ff);
    assert_eq!(
        MacInterface::from_field(config.interface()),
        MacInterface::AccessPoint
    );
    for other in [0, 2, 3] {
        assert_eq!(control.read_config(bank(other)).aifsn().get(), 0);
    }
}

#[test]
fn queue_publication_and_detach_edges_touch_only_valid_and_enable() {
    let before = HostWords::filled(0x0f0f_0f0f);
    let mut memory = before.clone();
    {
        let mut owners = memory.owners();
        let control = &mut owners.registers.mac.wifi_mac_tx_queue_control;
        publish_queue(control, 0);
        assert!(queue_enabled(control, 0) && queue_valid(control, 0));
        invalidate_queue(control, 0);
        assert!(queue_enabled(control, 0) && !queue_valid(control, 0));
        disable_queue(control, 0);
        assert!(!queue_enabled(control, 0) && !queue_valid(control, 0));
        let read = control.read_control_n(bank(0));
        assert_eq!(read.descriptor_address_low().get(), 0x0f0f_0f0f & 0xf_ffff);
    }
    assert_eq!(before.changes(&memory).len(), 1);
}

#[test]
fn protection_replaces_both_software_requests() {
    let mut memory = HostWords::filled(0);
    let mut owners = memory.owners();
    let control = &mut owners.registers.mac.wifi_mac_tx_queue_control;
    configure_protection(control, 2, MacTxProtection::RtsCts);
    let protection = control.read_protection(bank(2));
    assert!(protection.software_rts() && !protection.software_cts());
    configure_protection(control, 2, MacTxProtection::CtsToSelf);
    let protection = control.read_protection(bank(2));
    assert!(!protection.software_rts() && protection.software_cts());
    configure_protection(control, 2, MacTxProtection::None);
    let protection = control.read_protection(bank(2));
    assert!(!protection.software_rts() && !protection.software_cts());
}
