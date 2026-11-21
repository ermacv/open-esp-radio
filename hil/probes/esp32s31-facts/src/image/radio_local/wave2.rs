//! Isolated local producers, RF gates closed and no DMA publication.
use super::*;

/// `enabled` for the lane `own` when it is the one chosen, else disabled.
const fn only(lane: u32, own: u32, enabled: EventEnable) -> EventEnable {
    if lane == own {
        enabled
    } else {
        EventEnable::Disabled
    }
}
fn wifi_enable(s: &mut Session<'_>, lane: u32, enabled: EventEnable) {
    wifi_mask(s, EventEnable::Disabled);
    s.radio.power.modify_enable(|w| {
        w.set_tsf_timer_2(only(lane, 2, enabled))
            .set_tsf_timer_3(only(lane, 3, enabled))
            .set_tbtt_0(only(lane, 4, enabled));
    });
    oer_portable_riscv::fence();
}
fn wifi_observe(s: &Session<'_>, tag: u32) -> u32 {
    let r = s.radio.power.read_raw();
    let bits =
        u32::from(r.tsf_timer_2()) + 2 * u32::from(r.tsf_timer_3()) + 4 * u32::from(r.tbtt_0());
    emit(
        tag,
        bits,
        s.radio.power.sample_mac_power_interrupt().bits(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_3()
            .int_status_3(),
    );
    bits
}
fn station(s: &mut Session<'_>, tag: u32) -> u64 {
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_snapshot_station_tsf(Snapshot::Selected);
    });
    let low = s.radio.load.read_snapshot_low().value();
    let high = s.radio.load.read_snapshot_high().value();
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_snapshot_station_tsf(Snapshot::Unselected);
    });
    emit(tag, low, high, s.radio.local_time.read_counter().value());
    (u64::from(high) << 32) | u64::from(low)
}
fn timer_counter(s: &mut Session<'_>, n: u32, tag: u32) -> u64 {
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_snapshot_tsf2(if n == 2 {
            Snapshot::Selected
        } else {
            Snapshot::Unselected
        })
        .set_snapshot_tsf3(if n == 3 {
            Snapshot::Selected
        } else {
            Snapshot::Unselected
        });
    });
    let low = s.radio.load.read_snapshot_low().value();
    let high = s.radio.load.read_snapshot_high().value();
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_snapshot_tsf2(Snapshot::Unselected)
            .set_snapshot_tsf3(Snapshot::Unselected);
    });
    emit(tag, low, high, n);
    (u64::from(high) << 32) | u64::from(low)
}
fn wifi_pending(s: &mut Session<'_>, lane: u32) {
    // Both acknowledgement paths require a positive enabled sample. Preserve
    // that affine sample across mask; no fabricated raw clear transaction.
    let sample = s.radio.power.sample_mac_power_interrupt();
    wifi_enable(s, lane, EventEnable::Disabled);
    wifi_observe(s, 0x22030);
    s.radio.power.acknowledge_mac_power_interrupt(sample);
    oer_portable_riscv::fence();
    wifi_observe(s, 0x22040);
    wifi_enable(s, lane, EventEnable::Enabled);
    window();
    wifi_observe(s, 0x22050);
}
fn timer(s: &mut Session<'_>, n: u32) {
    wifi_prepare(s);
    wifi_enable(s, n, EventEnable::Enabled);
    s.radio.power.clear_all_mac_power_interrupts(); // Also masks every lane.
    wifi_enable(s, n, EventEnable::Enabled);
    // SOURCE: BLOB_S31_TSF_DOMAIN_MODES_20261027. These are two-stage
    // vendor gate/mode transactions, with NAN mode4 and BTWT mode1.
    s.radio.load.write_value_low(|w| {
        w.set_value(0);
    });
    s.radio.load.write_value_high(|w| {
        w.set_value(0);
    });
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_load_station_tsf(true);
    });
    window();
    s.radio.aux.modify_nan_control(|w| {
        w.set_tsf_enable_low(true).set_tsf_enable_high(true);
    });
    s.radio.aux.modify_nan_control(|w| {
        w.set_tsf_mode(pac::modem_pwr::nan_control::TsfMode::Enabled);
    });
    s.radio.aux.modify_btwt_control(|w| {
        w.set_tsf_enable_low(true).set_tsf_enable_high(true);
    });
    s.radio.aux.modify_btwt_control(|w| {
        w.set_tsf_mode(pac::modem_pwr::btwt_control::TsfMode::Enabled);
    });
    let Ok(i) = Index::new(n as usize) else {
        return;
    };
    s.radio.timer.modify_control(i, |w| {
        w.set_timer_enable(Run::Stopped).set_wakeup_enable(false);
    });
    s.radio.power.clear_all_mac_power_interrupts();
    wifi_enable(s, n, EventEnable::Enabled);
    wifi_observe(s, 0x22000);
    // SOURCE: BLOB_S31_TSF_COUNTER_SELECT_20261027. Load and observe the
    // selected generic counter itself, rather than using station time as a proxy.
    s.radio.load.write_value_low(|w| {
        w.set_value(0);
    });
    s.radio.load.write_value_high(|w| {
        w.set_value(0);
    });
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_load_tsf2(n == 2).set_load_tsf3(n == 3);
    });
    window();
    let target = timer_counter(s, n, 0x22004).wrapping_add(256);
    s.radio.timer.modify_target_n(i, |w| {
        w.set_value(target as u32);
    });
    s.radio.timer.modify_control(i, |w| {
        w.set_target_high(Bits::truncate((target >> 32) as u32))
            .set_timer_enable(Run::Running);
    });
    let control = s.radio.timer.read_control(i);
    emit(
        0x22006,
        s.radio.timer.read_target_n(i).value(),
        control.target_high().get(),
        control.timer_enable().bits() + 2 * u32::from(control.wakeup_enable()),
    );
    let start = oer_portable_riscv::cycle32();
    let mut r = s.radio.power.read_raw();
    while !(if n == 2 {
        r.tsf_timer_2()
    } else {
        r.tsf_timer_3()
    }) && oer_portable_riscv::cycle32().wrapping_sub(start) < LOCAL_LIMIT
    {
        r = s.radio.power.read_raw();
    }
    emit(
        0x22010,
        n,
        oer_portable_riscv::cycle32().wrapping_sub(start),
        0,
    );
    timer_counter(s, n, 0x22014);
    s.radio.timer.modify_control(i, |w| {
        w.set_timer_enable(Run::Stopped);
    });
    if wifi_observe(s, 0x22020) != 0 {
        wifi_pending(s, n);
    }
    wifi_enable(s, n, EventEnable::Disabled);
    s.radio.power.clear_all_mac_power_interrupts();
    wifi_observe(s, 0x220f0);
}
pub(in crate::image) fn wifi2(s: &mut Session<'_>) {
    timer(s, 2);
}
pub(in crate::image) fn wifi3(s: &mut Session<'_>) {
    timer(s, 3);
}
pub(in crate::image) fn tbtt(s: &mut Session<'_>) {
    wifi_prepare(s);
    wifi_enable(s, 4, EventEnable::Disabled);
    s.radio.power.clear_all_mac_power_interrupts();
    // SOURCE: HAL start_station_tbtt / BLOB_LIBPP_HAL_TSF_TBTT_20260927.
    // Follow the complete local schedule order, with a fresh future TSF.
    let target = station(s, 0x22004).wrapping_add(3072);
    s.radio.rtc.modify_sta_tsf_control(|w| {
        w.set_sta_tbtt_enable(Run::Stopped);
    });
    s.radio.rtc.modify_sta_tsf_control(|w| {
        w.set_sta_tbtt_enable(Run::Running);
    });
    s.radio.rtc.modify_sta_tbtt_interval(|w| {
        w.set_interval_tsf_bits_35_10(Bits::constant::<16>());
    });
    s.radio.tbtt.modify_target(|w| {
        w.set_tsf_bits_35_10(Bits::truncate((target >> 10) as u32));
    });
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_load_station_tbtt_target(true);
    });
    s.radio.rtc.modify_sta_tsf_control(|w| {
        w.set_sta_tbtt_ahead_time(0);
    });
    s.radio.rtc.modify_sta_light_sleep_wake_ahead(|w| {
        w.set_time(0);
    });
    s.radio.rtc.modify_soc_wakeup_clear(|w| {
        w.set_clear_request(true);
    });
    s.radio.status.modify_wakeup_signal_clear(|w| {
        w.set_clear(true);
    });
    wifi_enable(s, 4, EventEnable::Enabled);
    wifi_observe(s, 0x22000);
    let start = oer_portable_riscv::cycle32();
    while !s.radio.power.read_raw().tbtt_0()
        && oer_portable_riscv::cycle32().wrapping_sub(start) < LOCAL_LIMIT
    {}
    emit(
        0x22010,
        4,
        oer_portable_riscv::cycle32().wrapping_sub(start),
        0,
    );
    station(s, 0x22014);
    s.radio.rtc.modify_sta_tsf_control(|w| {
        w.set_sta_tbtt_enable(Run::Stopped);
    });
    if wifi_observe(s, 0x22020) != 0 {
        wifi_pending(s, 4);
    }
    wifi_enable(s, 4, EventEnable::Disabled);
    s.radio.power.clear_all_mac_power_interrupts();
    wifi_observe(s, 0x220f0);
}
