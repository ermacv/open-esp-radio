//! The TSF transactions over host memory.

use oer_esp32s31_pac::modem_pwr::{
    snapshot_high, snapshot_low, soc_wakeup_clear, sta_light_sleep_wake_ahead, sta_tbtt_interval,
    sta_tsf_control::{self, StaTsfMode},
    target, tsf_load_control, value_high, value_low, wakeup_signal_clear,
};

use super::StaTbttSchedule;
use crate::ieee80211::registers::HostWords;

/// The (part, offset) of each changed word.
fn changed(before: &HostWords, after: &HostWords) -> std::vec::Vec<(&'static str, usize)> {
    before
        .changes(after)
        .into_iter()
        .map(|(part, offset, _)| (part, offset))
        .collect()
}

#[test]
fn a_station_tsf_is_loaded_low_word_high_word_then_strobe() {
    let mut words = HostWords::filled(0);
    let before = words.clone();
    words
        .owners()
        .registers
        .set_station_tsf(0x1122_3344_5566_7788);
    let changes = before.changes(&words);
    assert_eq!(
        changed(&before, &words),
        [
            ("wifi_mac_tsf_load", tsf_load_control::OFFSET),
            ("wifi_mac_tsf_load", value_low::OFFSET),
            ("wifi_mac_tsf_load", value_high::OFFSET),
        ]
    );
    assert_eq!(changes.get(1).map(|change| change.2), Some(0x5566_7788));
    assert_eq!(changes.get(2).map(|change| change.2), Some(0x1122_3344));
    let owners = words.owners();
    let control = owners
        .registers
        .mac
        .wifi_mac_tsf_load
        .read_tsf_load_control();
    assert!(control.load_station_tsf());
    assert!(!(control.load_softap_tsf() || control.snapshot_station_tsf()));
}

#[test]
fn snapshots_select_read_and_release_without_leaving_a_change() {
    let mut words = HostWords::filled(0);
    for (offset, value) in [
        (snapshot_low::OFFSET, 0x0bad_cafe),
        (snapshot_high::OFFSET, 0x0000_0123),
    ] {
        if let Some(word) = words.wifi_mac_tsf_load.get_mut(offset / 4) {
            *word = value;
        }
    }
    let before = words.clone();
    let (station, access_point) = {
        let mut owners = words.owners();
        (
            owners.registers.station_tsf(),
            owners.registers.access_point_tsf(),
        )
    };
    assert_eq!(station, 0x0000_0123_0bad_cafe);
    assert_eq!(access_point, 0x0000_0123_0bad_cafe);
    assert_eq!(before.changes(&words), []);
}

#[test]
fn the_station_tsf_scheduler_is_enabled_in_two_edges() {
    let mut words = HostWords::filled(0);
    let before = words.clone();
    words.owners().registers.enable_station_tsf();
    assert_eq!(
        changed(&before, &words),
        [("wifi_mac_rtc_timer_update", sta_tsf_control::OFFSET)]
    );
    let owners = words.owners();
    let control = owners
        .registers
        .mac
        .wifi_mac_rtc_timer_update
        .read_sta_tsf_control();
    assert!(control.sta_tsf_enable_low() && control.sta_tsf_enable_high());
    assert_eq!(control.sta_tsf_mode_variant(), Some(StaTsfMode::Enabled));
    assert!(!control.sta_tbtt_enable());
}

#[test]
fn a_tbtt_schedule_programs_every_word_of_hal_set_sta_tbtt() {
    let mut words = HostWords::filled(0);
    let before = words.clone();
    words
        .owners()
        .registers
        .start_station_tbtt(StaTbttSchedule {
            first_tbtt_tsf: 0x0000_000f_ffff_fc00 | 0x3ff,
            interval_micros: 102_400,
            ahead_micros: 0x1234,
            wake_ahead_micros: 0x0567,
        });
    assert_eq!(
        changed(&before, &words),
        [
            ("wifi_mac_rtc_timer_update", soc_wakeup_clear::OFFSET),
            (
                "wifi_mac_rtc_timer_update",
                sta_light_sleep_wake_ahead::OFFSET
            ),
            ("wifi_mac_rtc_timer_update", sta_tsf_control::OFFSET),
            ("wifi_mac_rtc_timer_update", sta_tbtt_interval::OFFSET),
            ("wifi_mac_sta_tbtt_target", target::OFFSET),
            ("wifi_mac_tsf_load", tsf_load_control::OFFSET),
            ("wifi_mac_tsf_status", wakeup_signal_clear::OFFSET),
        ]
    );
    let owners = words.owners();
    let mac = &owners.registers.mac;
    let rtc = &mac.wifi_mac_rtc_timer_update;
    let control = rtc.read_sta_tsf_control();
    assert!(control.sta_tbtt_enable());
    assert_eq!(control.sta_tbtt_ahead_time(), 0x1234);
    assert_eq!(rtc.read_sta_light_sleep_wake_ahead().time(), 0x0567);
    assert_eq!(
        rtc.read_sta_tbtt_interval().interval_tsf_bits_35_10().get(),
        102_400 >> 10
    );
    assert!(rtc.read_soc_wakeup_clear().clear_request());
    // TSF bits 35:10 of the first TBTT; bits 31:26 of the word are kept.
    let target = mac.wifi_mac_sta_tbtt_target.read_target();
    assert_eq!(target.tsf_bits_35_10().get(), 0x03ff_ffff);
    assert_eq!(target.bits_26_31_opaque().get(), 0);
    assert!(
        mac.wifi_mac_tsf_load
            .read_tsf_load_control()
            .load_station_tbtt_target()
    );
    assert!(mac.wifi_mac_tsf_status.read_wakeup_signal_clear().clear());
}

#[test]
fn stopping_the_tbtt_clears_only_its_enable() {
    let mut words = HostWords::filled(u32::MAX);
    let before = words.clone();
    words.owners().registers.stop_station_tbtt();
    assert_eq!(
        changed(&before, &words),
        [("wifi_mac_rtc_timer_update", sta_tsf_control::OFFSET)]
    );
    let owners = words.owners();
    let control = owners
        .registers
        .mac
        .wifi_mac_rtc_timer_update
        .read_sta_tsf_control();
    assert!(!control.sta_tbtt_enable());
    assert!(control.sta_tsf_enable_low() && control.sta_tsf_enable_high());
}

#[test]
fn the_tbtt_leads_and_interval_replace_their_fields() {
    let mut words = HostWords::filled(u32::MAX);
    {
        let mut owners = words.owners();
        owners.registers.set_station_tbtt_ahead(0x0102, 0x0304);
        owners.registers.set_station_tbtt_interval(0x0040_0000);
    }
    let owners = words.owners();
    let rtc = &owners.registers.mac.wifi_mac_rtc_timer_update;
    let control = rtc.read_sta_tsf_control();
    assert_eq!(control.sta_tbtt_ahead_time(), 0x0102);
    assert!(control.sta_tbtt_enable());
    let wake = rtc.read_sta_light_sleep_wake_ahead();
    assert_eq!(wake.time(), 0x0304);
    assert_eq!(wake.bits_0_15_opaque(), 0xffff);
    let interval = rtc.read_sta_tbtt_interval();
    assert_eq!(interval.interval_tsf_bits_35_10().get(), 0x0040_0000 >> 10);
    assert_eq!(interval.bits_30_31_opaque().get(), 0b11);
}
