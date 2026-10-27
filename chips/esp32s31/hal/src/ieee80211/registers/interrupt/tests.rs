use crate::ieee80211::registers::{
    HostWords, MacInterruptMask, interrupt::MacInterruptEnableState,
};

#[test]
fn rx_selection_keeps_only_sampled_selected_lanes() {
    use super::MacRxInterrupts;
    for selected in [
        MacRxInterrupts::NONE,
        MacRxInterrupts::ALL,
        MacRxInterrupts::AUXILIARY_5,
        MacRxInterrupts::SUCCESS,
        MacRxInterrupts::AUXILIARY_24,
    ] {
        for initial in [0, u32::MAX] {
            let mut words = HostWords::filled(initial);
            let sample = words
                .owners()
                .mac_interrupt
                .mac_interrupt_status()
                .retain_rx_causes(selected);
            let expected = if initial == 0 {
                MacRxInterrupts::NONE
            } else {
                selected
            };
            assert_eq!(sample.rx_causes(), expected);
            assert!(!sample.observation().has_unhandled_event());
            let bits = sample.bits();
            assert_eq!(
                bits.count_ones(),
                u32::from(expected.auxiliary_5)
                    + u32::from(expected.success)
                    + u32::from(expected.auxiliary_24)
            );
            let before = words.clone();
            words
                .owners()
                .mac_interrupt
                .acknowledge_mac_interrupts(sample);
            assert_eq!(
                values(&before.changes(&words), "interrupt"),
                if initial == 0 {
                    std::vec![]
                } else {
                    std::vec![bits]
                }
            );
            assert!(
                before
                    .changes(&words)
                    .iter()
                    .all(|(part, _, _)| *part == "interrupt")
            );
        }
    }
}

#[test]
fn individual_rx_enables_preserve_unrelated_delivery_and_raw_observation() {
    use super::MacRxInterrupts;
    let mut words = HostWords::filled(u32::MAX);
    let mut owners = words.owners();
    owners
        .mac_interrupt
        .set_mac_rx_interrupt_enable(MacRxInterrupts::AUXILIARY_24);
    assert_eq!(
        owners.mac_interrupt.mac_rx_interrupt_enable(),
        MacRxInterrupts::AUXILIARY_24
    );
    assert!(owners.mac_interrupt.mac.read_enable().tx_complete());
    assert!(owners.mac_interrupt.mac.read_enable().fatal_assertion());
    let raw = owners.mac_interrupt.mac_interrupt_raw();
    assert_eq!(raw.bits(), u32::MAX);
    assert_eq!(raw.fatal_assertion_raw(), Some(raw.bits()));
}

/// The value each changed word of one part holds, in memory order.
fn values(changes: &[(&'static str, usize, u32)], part: &str) -> std::vec::Vec<u32> {
    changes
        .iter()
        .filter(|(changed, _, _)| *changed == part)
        .map(|(_, _, value)| *value)
        .collect()
}

#[test]
fn publishing_the_station_mask_writes_its_image_and_reads_back_as_station() {
    let before = HostWords::filled(0);
    let mut after = before.clone();
    {
        let mut owners = after.owners();
        owners
            .mac_interrupt
            .publish_mac_interrupt_mask(MacInterruptMask::STATION);
        let enable = owners.mac_interrupt.mac.read_enable();
        assert!(enable.tx_complete() && !enable.bss_color_collision());
        assert_eq!(
            owners.mac_interrupt.mac_interrupt_enable(),
            MacInterruptEnableState::Station
        );
    }
    let changes = before.changes(&after);
    assert_eq!(
        values(&changes, "interrupt"),
        [MacInterruptMask::STATION.bits()]
    );
    assert_eq!(changes.len(), 1);
}

#[test]
fn the_power_bank_enables_the_station_tbtt_alone() {
    let before = HostWords::filled(0);
    let mut after = before.clone();
    after
        .owners()
        .power_interrupt
        .enable_station_tbtt_interrupt();
    let changes = before.changes(&after);
    assert_eq!(values(&changes, "power_interrupt"), [1 << 3]);
    assert_eq!(changes.len(), 1);
}

#[test]
fn masking_a_bank_writes_the_zero_enable_and_clears_nothing() {
    let before = HostWords::filled(u32::MAX);
    let mut after = before.clone();
    {
        let mut owners = after.owners();
        owners.mac_interrupt.mask_all_mac_interrupts();
        owners.power_interrupt.mask_all_power_interrupts();
        assert_eq!(
            owners.mac_interrupt.mac_interrupt_enable(),
            MacInterruptEnableState::Disabled
        );
    }
    let changes = before.changes(&after);
    // One enable word per bank, each now zero; no clear word is stored.
    assert_eq!(values(&changes, "interrupt"), [0]);
    assert_eq!(values(&changes, "power_interrupt"), [0]);
    assert_eq!(changes.len(), 2);
}

#[test]
fn clearing_power_stores_the_zero_enable_then_the_whole_clear_word() {
    let before = HostWords::filled(0x5a5a_5a5a);
    let mut after = before.clone();
    after.owners().power_interrupt.clear_all_power_interrupts();
    let changes = before.changes(&after);
    assert!(values(&changes, "interrupt").is_empty());
    assert_eq!(values(&changes, "power_interrupt"), [0, u32::MAX]);
}

#[test]
fn raw_acknowledgement_does_not_select_a_fatal_cause_arriving_after_the_sample() {
    let mut words = HostWords::filled(u32::MAX);
    words.interrupt[oer_esp32s31_pac::wifi_mac::raw::OFFSET / size_of::<u32>()] &=
        !oer_esp32s31_pac::wifi_mac::raw::FATAL_ASSERTION_MASK;
    let sample = {
        let mut owners = words.owners();
        owners.mac_interrupt.mask_all_mac_interrupts();
        owners.mac_interrupt.mac_interrupt_raw()
    };
    assert_eq!(sample.fatal_assertion_raw(), None);
    let sampled_bits = sample.0.bits();
    words.interrupt[oer_esp32s31_pac::wifi_mac::raw::OFFSET / size_of::<u32>()] |=
        oer_esp32s31_pac::wifi_mac::raw::FATAL_ASSERTION_MASK;
    assert!(
        words
            .owners()
            .mac_interrupt
            .mac_interrupt_raw()
            .fatal_assertion_raw()
            .is_some()
    );
    let before_clear = words.clone();
    words
        .owners()
        .mac_interrupt
        .acknowledge_mac_interrupt_raw(sample);
    // The host storage records the clear word; its exact value must be the
    // old sample, excluding the new fatal cause. W1C itself is modelled in MAC.
    assert_eq!(
        values(&before_clear.changes(&words), "interrupt"),
        [sampled_bits]
    );
}

#[test]
fn the_rx_delivery_group_masks_and_unmasks_alone() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners
        .mac_interrupt
        .publish_mac_interrupt_mask(MacInterruptMask::STATION);
    owners.mac_interrupt.mask_rx_delivery_interrupts();
    let enable = owners.mac_interrupt.mac.read_enable();
    assert!(!enable.rx_success() && !enable.rx_associated_auxiliary_5());
    assert!(enable.tx_complete(), "TX stays live");
    owners.mac_interrupt.unmask_rx_delivery_interrupts();
    assert_eq!(
        owners.mac_interrupt.mac_interrupt_enable(),
        MacInterruptEnableState::Station
    );
}

#[test]
fn an_empty_sample_acknowledges_nothing() {
    let before = HostWords::filled(0);
    let mut after = before.clone();
    {
        let mut owners = after.owners();
        let snapshot = owners.mac_interrupt.mac_interrupt_status();
        assert!(snapshot.observation().is_empty());
        owners.mac_interrupt.acknowledge_mac_interrupts(snapshot);
        let snapshot = owners.power_interrupt.power_interrupt_status();
        assert!(snapshot.observation().is_empty());
        owners
            .power_interrupt
            .acknowledge_power_interrupts(snapshot);
    }
    // The empty acknowledgements store zero over the zero clear words.
    assert!(before.changes(&after).is_empty());
}

#[test]
fn a_full_status_reads_every_cause() {
    let mut words = HostWords::filled(u32::MAX);
    let owners = words.owners();
    let observation = owners.mac_interrupt.mac_interrupt_status().observation();
    assert!(observation.work_events().tx_complete());
    assert!(observation.work_events().bss_color_collision());
    assert!(observation.work_events().rx_success());
    assert!(observation.work_events().tx_timeout());
    assert!(observation.has_auxiliary_event() && observation.has_unhandled_event());
    assert_eq!(
        owners
            .mac_interrupt
            .mac_interrupt_raw()
            .fatal_assertion_raw(),
        Some(u32::MAX)
    );
    let power = owners
        .power_interrupt
        .power_interrupt_status()
        .observation();
    assert!(power.sta_tbtt() && power.has_unhandled_event());
    assert!(power.tsf_timer_0() && power.tsf_timer_3());
}

#[test]
fn preparing_a_connected_station_clears_the_beacon_filter_control_alone() {
    let before = HostWords::filled(u32::MAX);
    let mut after = before.clone();
    {
        let mut owners = after.owners();
        let _prepared = owners.registers.prepare_connected_sta_without_power_save();
        let control = owners
            .registers
            .mac
            .wifi_mac_sta_beacon_filter
            .read_sta_beacon_filter_control();
        assert!(!control.beacon_filter_enable());
        assert!(!control.broadcast_wakeup_enable());
        assert!(!control.unicast_wakeup_enable());
        assert_ne!(control.bits_3_31_opaque().get(), 0, "kept");
    }
    // The interrupt enable word is the handler's: it is not written here.
    let changes = before.changes(&after);
    assert!(values(&changes, "interrupt").is_empty());
    assert_eq!(changes.len(), 1);
}

#[test]
fn fatal_raw_is_visible_while_status_and_delivery_are_masked() {
    let mut words = HostWords::filled(0);
    words.interrupt[oer_esp32s31_pac::wifi_mac::raw::OFFSET / size_of::<u32>()] =
        oer_esp32s31_pac::wifi_mac::raw::FATAL_ASSERTION_MASK;
    let mut owners = words.owners();
    owners.mac_interrupt.mask_all_mac_interrupts();
    assert!(
        owners
            .mac_interrupt
            .mac_interrupt_status()
            .observation()
            .is_empty()
    );
    assert_eq!(
        owners.mac_interrupt.mac_interrupt_enable(),
        MacInterruptEnableState::Disabled
    );
    assert!(
        owners
            .mac_interrupt
            .mac_interrupt_raw()
            .fatal_assertion_raw()
            .is_some()
    );
}
