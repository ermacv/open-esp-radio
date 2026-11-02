//! Isolated local producers, RF gates closed and no DMA publication.
use super::*;

fn wifi_enable(s: &mut Session<'_>, lane: u32, enabled: bool) {
    wifi_mask(s, false);
    s.radio.power.modify_enable(|w| {
        w.set_tsf_timer_2(lane == 2 && enabled)
            .set_tsf_timer_3(lane == 3 && enabled)
            .set_tbtt_0(lane == 4 && enabled);
    });
    oer_riscv::fence();
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
        w.set_snapshot_station_tsf(true);
    });
    let low = s.radio.load.read_snapshot_low().value();
    let high = s.radio.load.read_snapshot_high().value();
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_snapshot_station_tsf(false);
    });
    emit(tag, low, high, s.radio.local_time.read_counter().value());
    (u64::from(high) << 32) | u64::from(low)
}
fn timer_counter(s: &mut Session<'_>, n: u32, tag: u32) -> u64 {
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_snapshot_tsf2(n == 2).set_snapshot_tsf3(n == 3);
    });
    let low = s.radio.load.read_snapshot_low().value();
    let high = s.radio.load.read_snapshot_high().value();
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_snapshot_tsf2(false).set_snapshot_tsf3(false);
    });
    emit(tag, low, high, n);
    (u64::from(high) << 32) | u64::from(low)
}
fn wifi_pending(s: &mut Session<'_>, lane: u32) {
    // Both acknowledgement paths require a positive enabled sample. Preserve
    // that affine sample across mask; no fabricated raw clear transaction.
    let sample = s.radio.power.sample_mac_power_interrupt();
    wifi_enable(s, lane, false);
    wifi_observe(s, 0x22030);
    s.radio.power.acknowledge_mac_power_interrupt(sample);
    oer_riscv::fence();
    wifi_observe(s, 0x22040);
    wifi_enable(s, lane, true);
    window();
    wifi_observe(s, 0x22050);
}
fn timer(s: &mut Session<'_>, n: u32) {
    wifi_prepare(s);
    wifi_enable(s, n, true);
    s.radio.power.clear_all_mac_power_interrupts(); // Also masks every lane.
    wifi_enable(s, n, true);
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
        w.set_timer_enable(false).set_wakeup_enable(false);
    });
    s.radio.power.clear_all_mac_power_interrupts();
    wifi_enable(s, n, true);
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
            .set_timer_enable(true);
    });
    let control = s.radio.timer.read_control(i);
    emit(
        0x22006,
        s.radio.timer.read_target_n(i).value(),
        control.target_high().get(),
        u32::from(control.timer_enable()) + 2 * u32::from(control.wakeup_enable()),
    );
    let start = oer_riscv::cycle32();
    let mut r = s.radio.power.read_raw();
    while !(if n == 2 {
        r.tsf_timer_2()
    } else {
        r.tsf_timer_3()
    }) && oer_riscv::cycle32().wrapping_sub(start) < LOCAL_LIMIT
    {
        r = s.radio.power.read_raw();
    }
    emit(0x22010, n, oer_riscv::cycle32().wrapping_sub(start), 0);
    timer_counter(s, n, 0x22014);
    s.radio.timer.modify_control(i, |w| {
        w.set_timer_enable(false);
    });
    if wifi_observe(s, 0x22020) != 0 {
        wifi_pending(s, n);
    }
    wifi_enable(s, n, false);
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
    wifi_enable(s, 4, false);
    s.radio.power.clear_all_mac_power_interrupts();
    // SOURCE: HAL start_station_tbtt / BLOB_LIBPP_HAL_TSF_TBTT_20260927.
    // Follow the complete local schedule order, with a fresh future TSF.
    let target = station(s, 0x22004).wrapping_add(3072);
    s.radio.rtc.modify_sta_tsf_control(|w| {
        w.set_sta_tbtt_enable(false);
    });
    s.radio.rtc.modify_sta_tsf_control(|w| {
        w.set_sta_tbtt_enable(true);
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
    wifi_enable(s, 4, true);
    wifi_observe(s, 0x22000);
    let start = oer_riscv::cycle32();
    while !s.radio.power.read_raw().tbtt_0()
        && oer_riscv::cycle32().wrapping_sub(start) < LOCAL_LIMIT
    {}
    emit(0x22010, 4, oer_riscv::cycle32().wrapping_sub(start), 0);
    station(s, 0x22014);
    s.radio.rtc.modify_sta_tsf_control(|w| {
        w.set_sta_tbtt_enable(false);
    });
    if wifi_observe(s, 0x22020) != 0 {
        wifi_pending(s, 4);
    }
    wifi_enable(s, 4, false);
    s.radio.power.clear_all_mac_power_interrupts();
    wifi_observe(s, 0x220f0);
}

fn zb_enable(s: &mut Session<'_>, enabled: bool) {
    zb_mask(s, false);
    s.radio.zb.ieee802154_mac_task.modify_event_enable(|w| {
        w.set_rx_abort(enabled).set_ed_done(enabled);
    });
    oer_riscv::fence();
}
fn zb_observe(s: &Session<'_>, tag: u32) -> u32 {
    let r = s.radio.zb.ieee802154_mac_irq.read_event_status();
    let e = s.radio.zb.ieee802154_mac_task.read_event_enable();
    let bits = u32::from(r.ed_done()) + 2 * u32::from(r.rx_abort());
    emit(
        tag,
        bits,
        u32::from(e.ed_done()) + 2 * u32::from(e.rx_abort()),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_4()
            .int_status_4(),
    );
    emit(
        tag + 1,
        s.radio
            .zb
            .ieee802154_mac_task
            .read_rx_status()
            .abort_reason()
            .get(),
        s.radio.idle.read_status().txrx_state().get(),
        0,
    );
    bits
}
fn ed_leaf(s: &mut Session<'_>, stop: bool) {
    use pac::ieee802154::command::Opcode;
    ieee_prepare(s);
    zb_enable(s, true);
    zb_clear(s);
    // SOURCE: public vendor LL disable-coexistence helper (HAL lifecycle).
    // RF stays closed. This removes the observed ED_COEX_REJECT prerequisite.
    s.radio.zb.ieee802154_mac_task.modify_coex_pti(|w| {
        w.set_txrx_pti(Bits::constant::<3>());
    });
    s.radio.zb.ieee802154_mac_task.modify_coex_pti(|w| {
        w.set_ack_pti(Bits::constant::<3>());
    });
    let pti = s.radio.zb.ieee802154_mac_task.read_coex_pti();
    emit(0x22302, pti.txrx_pti().get(), pti.ack_pti().get(), 0);
    s.radio.zb.ieee802154_mac_task.modify_rx_abort_enable(|w| {
        w.set_ed_stop(true)
            .set_ed_abort(true)
            .set_ed_coexistence_reject(true);
    });
    s.radio.zb.ieee802154_mac_task.modify_ed_duration(|w| {
        w.set_duration(Bits::truncate(if stop { 100_000 } else { 32 }))
            .set_delay(Bits::constant::<0>());
    });
    s.radio.zb.ieee802154_mac_task.modify_ed_config(|w| {
        w.set_ed_sample_rate(pac::ieee802154::ed_config::EdSampleRate::OnePerUs)
            .set_ed_sample_mode(pac::ieee802154::ed_config::EdSampleMode::Maximum);
    });
    zb_observe(s, 0x22300);
    s.radio
        .zb
        .ieee802154_mac_task
        .write_command(Opcode::EdStart, |_| {});
    if stop {
        window();
        zb_observe(s, 0x22310);
        s.radio
            .zb
            .ieee802154_mac_task
            .write_command(Opcode::Stop, |_| {});
    }
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < LOCAL_LIMIT {
        let r = s.radio.zb.ieee802154_mac_irq.read_event_status();
        if r.ed_done() || r.rx_abort() {
            break;
        }
    }
    emit(
        0x22318,
        u32::from(stop),
        oer_riscv::cycle32().wrapping_sub(start),
        0,
    );
    let positive = zb_observe(s, 0x22320);
    s.radio
        .zb
        .ieee802154_mac_task
        .write_command(Opcode::Stop, |_| {});
    if positive != 0 {
        let sample = s.radio.zb.ieee802154_mac_irq.sample_event_status();
        zb_enable(s, false);
        zb_observe(s, 0x22330);
        zb_clear(s); // A masked zero sample, independently from retained sample.
        zb_enable(s, true);
        zb_observe(s, 0x22340);
        zb_enable(s, false);
        s.radio
            .zb
            .ieee802154_mac_irq
            .acknowledge_event_status(sample);
        oer_riscv::fence();
        zb_observe(s, 0x22350);
        zb_enable(s, true);
        zb_observe(s, 0x22360);
    }
    // Independent enabled-clear case with a freshly started producer.
    s.radio
        .zb
        .ieee802154_mac_task
        .write_command(Opcode::EdStart, |_| {});
    if stop {
        window();
        s.radio
            .zb
            .ieee802154_mac_task
            .write_command(Opcode::Stop, |_| {});
    }
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < LOCAL_LIMIT {
        let r = s.radio.zb.ieee802154_mac_irq.read_event_status();
        if r.ed_done() || r.rx_abort() {
            break;
        }
    }
    zb_observe(s, 0x22370);
    s.radio
        .zb
        .ieee802154_mac_task
        .write_command(Opcode::Stop, |_| {});
    zb_clear(s);
    oer_riscv::fence();
    zb_observe(s, 0x22380);
    zb_clear(s);
    zb_enable(s, false);
    zb_observe(s, 0x223f0);
}
pub(in crate::image) fn ed(s: &mut Session<'_>) {
    ed_leaf(s, false);
}
pub(in crate::image) fn ed_stop(s: &mut Session<'_>) {
    ed_leaf(s, true);
}
