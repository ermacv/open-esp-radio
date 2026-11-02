//! PB-1c and the timer-only PB-1d route control. RF and CPU admission stay off.
#![forbid(unsafe_code)]
use super::*;
use crate::image::irq_observer;

// +8 TU fits in 20 ms at the independently checked 40 MHz CPU clock.
const DEADLINE: u32 = 800_000;
#[derive(Clone, Copy)]
enum Cause {
    Timer(u8),
    Tbtt(u8),
}
impl Cause {
    fn domain(self) -> u8 {
        match self {
            Self::Timer(n) | Self::Tbtt(n) => n,
        }
    }
    fn bit(self) -> u32 {
        match self {
            Self::Timer(2) => pac::modem_pwr::raw::TSF_TIMER_2_MASK,
            Self::Timer(_) => pac::modem_pwr::raw::TSF_TIMER_3_MASK,
            Self::Tbtt(0) => pac::modem_pwr::raw::TBTT_0_MASK,
            Self::Tbtt(1) => pac::modem_pwr::raw::TBTT_1_MASK,
            Self::Tbtt(2) => pac::modem_pwr::raw::TBTT_2_MASK,
            Self::Tbtt(_) => pac::modem_pwr::raw::TBTT_3_MASK,
        }
    }
}
fn raw(s: &Session<'_>) -> u32 {
    let r = s.radio.power.read_raw();
    pack([
        r.tbtt_3(),
        r.tbtt_2(),
        r.tbtt_1(),
        r.tbtt_0(),
        r.tsf_timer_3(),
        r.tsf_timer_2(),
        r.tsf_timer_1(),
        r.tsf_timer_0(),
    ])
}
fn pack<const N: usize>(v: [bool; N]) -> u32 {
    const {
        assert!(N <= u32::BITS as usize);
    }
    v.into_iter()
        .enumerate()
        .fold(0, |a, (n, b)| a | (u32::from(b) << n))
}
fn state(s: &Session<'_>, tag: u32) {
    let e = s.radio.power.read_enable();
    emit(
        tag,
        raw(s),
        s.radio.power.sample_mac_power_interrupt().bits(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_3()
            .int_status_3(),
    );
    emit(
        tag + 1,
        pack([
            e.tbtt_3(),
            e.tbtt_2(),
            e.tbtt_1(),
            e.tbtt_0(),
            e.tsf_timer_3(),
            e.tsf_timer_2(),
            e.tsf_timer_1(),
            e.tsf_timer_0(),
        ]),
        pack([
            s.radio
                .timer
                .read_control(Index::constant::<0>())
                .timer_enable(),
            s.radio
                .timer
                .read_control(Index::constant::<1>())
                .timer_enable(),
            s.radio
                .timer
                .read_control(Index::constant::<2>())
                .timer_enable(),
            s.radio
                .timer
                .read_control(Index::constant::<3>())
                .timer_enable(),
        ]),
        pack([
            s.radio.rtc.read_sta_tsf_control().sta_tbtt_enable(),
            s.radio.aux.read_softap_control().tbtt_enable(),
            s.radio.aux.read_nan_control().tbtt_enable(),
            s.radio.aux.read_btwt_control().tbtt_enable(),
        ]),
    );
}
fn enable(s: &mut Session<'_>, c: Cause, on: bool) {
    wifi_mask(s, false);
    s.radio.power.modify_enable(|w| match c {
        Cause::Timer(2) => {
            w.set_tsf_timer_2(on);
        }
        Cause::Timer(_) => {
            w.set_tsf_timer_3(on);
        }
        Cause::Tbtt(0) => {
            w.set_tbtt_0(on);
        }
        Cause::Tbtt(1) => {
            w.set_tbtt_1(on);
        }
        Cause::Tbtt(2) => {
            w.set_tbtt_2(on);
        }
        Cause::Tbtt(_) => {
            w.set_tbtt_3(on);
        }
    });
    oer_riscv::fence();
}
fn run(s: &mut Session<'_>, c: Cause, on: bool) {
    match c {
        Cause::Timer(n) => {
            if let Ok(i) = Index::new(usize::from(n)) {
                s.radio.timer.modify_control(i, |w| {
                    w.set_timer_enable(on);
                });
            }
        }
        Cause::Tbtt(0) => s.radio.rtc.modify_sta_tsf_control(|w| {
            w.set_sta_tbtt_enable(on);
        }),
        Cause::Tbtt(1) => s.radio.aux.modify_softap_control(|w| {
            w.set_tbtt_enable(on);
        }),
        Cause::Tbtt(2) => s.radio.aux.modify_nan_control(|w| {
            w.set_tbtt_enable(on);
        }),
        Cause::Tbtt(_) => s.radio.aux.modify_btwt_control(|w| {
            w.set_tbtt_enable(on);
        }),
    }
    oer_riscv::fence();
}
fn snapshot(s: &mut Session<'_>, n: u8, tag: u32) -> u64 {
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_snapshot_station_tsf(n == 0)
            .set_snapshot_softap_tsf(n == 1)
            .set_snapshot_tsf2(n == 2)
            .set_snapshot_tsf3(n == 3);
    });
    let low = s.radio.load.read_snapshot_low().value();
    let high = s.radio.load.read_snapshot_high().value();
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_snapshot_station_tsf(false)
            .set_snapshot_softap_tsf(false)
            .set_snapshot_tsf2(false)
            .set_snapshot_tsf3(false);
    });
    emit(tag, low, high, u32::from(n));
    (u64::from(high) << 32) | u64::from(low)
}
fn init(s: &mut Session<'_>) -> bool {
    irq_observer::inherited(s);
    if !irq_observer::control(s) || !crate::image::opaque::xtal_cpu(s) {
        emit(0x30ee, 1, 0, 0);
        return false;
    }
    wifi_prepare(s);
    // SOURCE: BLOB_S31_TSF_DOMAIN_MODES_20261027 and the HW26 complete prefix.
    s.radio.aux.modify_softap_control(|w| {
        w.set_tsf_enable(true).set_bit_30_opaque(true);
    });
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
    for n in 0..4 {
        run(s, Cause::Tbtt(n), false);
        if let Ok(i) = Index::new(usize::from(n)) {
            s.radio.timer.modify_control(i, |w| {
                w.set_timer_enable(false).set_wakeup_enable(false);
            });
        }
    }
    s.radio.power.clear_all_mac_power_interrupts();
    true
}
fn load(s: &mut Session<'_>, n: u8) {
    s.radio.load.write_value_low(|w| {
        w.set_value(0x10000);
    });
    s.radio.load.write_value_high(|w| {
        w.set_value(0);
    });
    s.radio.load.modify_tsf_load_control(|w| {
        w.set_load_station_tsf(n == 0)
            .set_load_softap_tsf(n == 1)
            .set_load_tsf2(n == 2)
            .set_load_tsf3(n == 3);
    });
    window();
}
fn program(s: &mut Session<'_>, c: Cause, tag: u32) -> u64 {
    stage(s, c, tag, false)
}
fn stage(s: &mut Session<'_>, c: Cause, tag: u32, running_tbtt: bool) -> u64 {
    run(s, c, false);
    if running_tbtt {
        run(s, c, true);
    }
    let now = snapshot(s, c.domain(), tag);
    let target = match c {
        Cause::Timer(_) => now.wrapping_add(0x400),
        Cause::Tbtt(_) => ((now >> 10).wrapping_add(8)) << 10,
    };
    match c {
        Cause::Timer(n) => {
            if let Ok(i) = Index::new(usize::from(n)) {
                s.radio.timer.modify_target_n(i, |w| {
                    w.set_value(target as u32);
                });
                s.radio.timer.modify_control(i, |w| {
                    w.set_target_high(Bits::truncate((target >> 32) as u32));
                });
            }
        }
        Cause::Tbtt(n) => {
            match n {
                0 => s.radio.rtc.modify_sta_tbtt_interval(|w| {
                    w.set_interval_tsf_bits_35_10(Bits::constant::<8>());
                }),
                1 => s.radio.aux.modify_softap_tbtt_interval(|w| {
                    w.set_interval_tsf_bits_35_10(Bits::constant::<8>());
                }),
                2 => s.radio.aux.modify_nan_tbtt_interval(|w| {
                    w.set_interval_tsf_bits_35_10(Bits::constant::<8>());
                }),
                _ => s.radio.aux.modify_btwt_tbtt_interval(|w| {
                    w.set_interval_tsf_bits_35_10(Bits::constant::<8>());
                }),
            }
            s.radio.tbtt.modify_target(|w| {
                w.set_tsf_bits_35_10(Bits::truncate((target >> 10) as u32));
            });
            s.radio.load.modify_tsf_load_control(|w| {
                w.set_load_station_tbtt_target(n == 0)
                    .set_load_tsf1_tbtt_target(n == 1)
                    .set_load_tsf2_tbtt_target(n == 2)
                    .set_load_tsf3_tbtt_target(n == 3);
            });
            match n {
                0 => s.radio.rtc.modify_sta_tsf_control(|w| {
                    w.set_sta_tbtt_ahead_time(0);
                }),
                1 => s.radio.aux.modify_softap_control(|w| {
                    w.set_tbtt_ahead_time(0);
                }),
                2 => s.radio.aux.modify_nan_control(|w| {
                    w.set_tbtt_ahead_time(0);
                }),
                _ => s.radio.aux.modify_btwt_control(|w| {
                    w.set_tbtt_ahead_time(0);
                }),
            }
            s.radio.rtc.modify_sta_light_sleep_wake_ahead(|w| {
                w.set_time(0);
            });
            s.radio.rtc.modify_soc_wakeup_clear(|w| {
                w.set_clear_request(true);
            });
            s.radio.status.modify_wakeup_signal_clear(|w| {
                w.set_clear(true);
            });
        }
    }
    emit(tag + 1, target as u32, (target >> 32) as u32, c.bit());
    target
}
fn wait(s: &Session<'_>, c: Cause, tag: u32) -> bool {
    let start = oer_riscv::cycle32();
    while raw(s) & c.bit() == 0 && oer_riscv::cycle32().wrapping_sub(start) < DEADLINE {}
    let positive = raw(s) & c.bit() != 0;
    emit(
        tag,
        u32::from(positive),
        oer_riscv::cycle32().wrapping_sub(start),
        c.bit(),
    );
    positive
}
fn fresh(s: &mut Session<'_>, c: Cause, on: bool, tag: u32) -> bool {
    let target = program(s, c, tag);
    enable(s, c, on);
    run(s, c, true);
    let positive = wait(s, c, tag + 2);
    run(s, c, false);
    let end = snapshot(s, c.domain(), tag + 3);
    state(s, tag + 4);
    if !positive || end < target {
        emit(0x31ee, tag, u32::from(positive), u32::from(end >= target));
        return false;
    }
    true
}
fn leaf(s: &mut Session<'_>, c: Cause) {
    if !init(s) {
        return;
    }
    load(s, c.domain());
    let first = snapshot(s, c.domain(), 0x3100);
    window();
    let second = snapshot(s, c.domain(), 0x3101);
    snapshot(s, 0, 0x3102); // Station reference is recorded separately for timers2/3.
    if second <= first {
        emit(0x31ee, 0, u32::from(c.domain()), 0);
        return;
    }
    state(s, 0x3104);
    if !fresh(s, c, true, 0x3110) {
        return;
    }
    let retained = s.radio.power.sample_mac_power_interrupt();
    enable(s, c, false);
    state(s, 0x3120);
    enable(s, c, true);
    state(s, 0x3130);
    enable(s, c, false);
    s.radio.power.acknowledge_mac_power_interrupt(retained);
    oer_riscv::fence();
    state(s, 0x3140);
    enable(s, c, true);
    window();
    state(s, 0x3150);
    if !fresh(s, c, true, 0x3160) {
        return;
    }
    let sample = s.radio.power.sample_mac_power_interrupt();
    s.radio.power.acknowledge_mac_power_interrupt(sample);
    oer_riscv::fence();
    state(s, 0x3170);
    if !fresh(s, c, false, 0x3180) {
        return;
    }
    enable(s, c, true);
    state(s, 0x3190);
    s.radio.power.clear_all_mac_power_interrupts();
    oer_riscv::fence();
    state(s, 0x31a0);
    // Keep run disabled while crossing a fresh programmed target.
    let target = program(s, c, 0x31b0);
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < DEADLINE {}
    let end = snapshot(s, c.domain(), 0x31b2);
    state(s, 0x31b4);
    emit(0x31b6, u32::from(end >= target), raw(s), c.bit());
    if !fresh(s, c, true, 0x31c0) {
        return;
    }
    s.radio.power.clear_all_mac_power_interrupts();
    state(s, 0x31d0);
    // Deliberately leave the producer running through the real project policy.
    program(s, c, 0x31e0);
    enable(s, c, true);
    run(s, c, true);
    if !wait(s, c, 0x31e2) {
        run(s, c, false);
        return;
    }
    s.radio.power.clear_all_mac_power_interrupts();
    state(s, 0x31e4);
    if let Cause::Timer(n) = c {
        let target = snapshot(s, n, 0x31e6).wrapping_add(0x400);
        if let Ok(i) = Index::new(usize::from(n)) {
            s.radio.timer.modify_target_n(i, |w| {
                w.set_value(target as u32);
            });
            s.radio.timer.modify_control(i, |w| {
                w.set_target_high(Bits::truncate((target >> 32) as u32));
            });
        }
        emit(0x31e7, target as u32, (target >> 32) as u32, c.bit());
    }
    let _ = wait(s, c, 0x31e8);
    state(s, 0x31ea);
    run(s, c, false);
    enable(s, c, true);
    state(s, 0x31ec);
    s.radio.power.clear_all_mac_power_interrupts();
    state(s, 0x31f0);
}

pub(in crate::image) fn timer2(s: &mut Session<'_>) {
    leaf(s, Cause::Timer(2));
}
pub(in crate::image) fn timer3(s: &mut Session<'_>) {
    leaf(s, Cause::Timer(3));
}
pub(in crate::image) fn tbtt0(s: &mut Session<'_>) {
    leaf(s, Cause::Tbtt(0));
}
pub(in crate::image) fn tbtt1(s: &mut Session<'_>) {
    leaf(s, Cause::Tbtt(1));
}
pub(in crate::image) fn tbtt2(s: &mut Session<'_>) {
    leaf(s, Cause::Tbtt(2));
}
pub(in crate::image) fn tbtt3(s: &mut Session<'_>) {
    leaf(s, Cause::Tbtt(3));
}
pub(in crate::image) fn distinct(s: &mut Session<'_>) {
    if !init(s) {
        return;
    }
    for n in [2, 3] {
        load(s, n);
        program(s, Cause::Timer(n), 0x3200 + u32::from(n) * 8);
        run(s, Cause::Timer(n), true);
    }
    s.radio.power.modify_enable(|w| {
        w.set_tsf_timer_2(true).set_tsf_timer_3(true);
    });
    for n in [2, 3] {
        if !wait(s, Cause::Timer(n), 0x3220 + u32::from(n)) {
            return;
        }
        run(s, Cause::Timer(n), false);
    }
    state(s, 0x3230);
    let selected = s
        .radio
        .power
        .sample_mac_power_interrupt()
        .retain_tsf_timer_3(false);
    s.radio.power.acknowledge_mac_power_interrupt(selected);
    oer_riscv::fence();
    state(s, 0x3240);
    s.radio.power.clear_all_mac_power_interrupts();
    state(s, 0x3250);
}
pub(in crate::image) fn route132(s: &mut Session<'_>) {
    irq_observer::inherited(s);
    if !irq_observer::control(s) {
        return;
    }
    ieee_prepare(s);
    zb_mask(s, false);
    zb_clear(s);
    zb_state(s, 0x3300);
    zb_start(s, false);
    zb_state(s, 0x3310);
    zb_mask(s, true);
    zb_state(s, 0x3320); // route132 remains parked, CPU MIE stays zero.
    let retained = s.radio.zb.ieee802154_mac_irq.sample_event_status();
    zb_mask(s, false);
    s.radio
        .zb
        .ieee802154_mac_irq
        .acknowledge_event_status(retained);
    zb_mask(s, true);
    window();
    zb_state(s, 0x3330);
    zb_mask(s, false);
}

pub(in crate::image) fn distinct_tbtt(s: &mut Session<'_>) {
    if !init(s) {
        return;
    }
    for (a, b, tag) in [(0, 1, 0x3600), (2, 3, 0x3700)] {
        for n in [a, b] {
            load(s, n);
            program(s, Cause::Tbtt(n), tag + u32::from(n) * 4);
            run(s, Cause::Tbtt(n), true);
        }
        s.radio.power.modify_enable(|w| {
            w.set_tbtt_0(a == 0)
                .set_tbtt_1(b == 1)
                .set_tbtt_2(a == 2)
                .set_tbtt_3(b == 3);
        });
        for n in [a, b] {
            if !wait(s, Cause::Tbtt(n), tag + 0x20 + u32::from(n)) {
                for k in [a, b] {
                    run(s, Cause::Tbtt(k), false);
                }
                return;
            }
        }
        for n in [a, b] {
            run(s, Cause::Tbtt(n), false);
        }
        state(s, tag + 0x30);
        let sample = s.radio.power.sample_mac_power_interrupt();
        let selected = if a == 0 {
            sample.retain_tbtt_1(false)
        } else {
            sample.retain_tbtt_3(false)
        };
        s.radio.power.acknowledge_mac_power_interrupt(selected);
        oer_riscv::fence();
        state(s, tag + 0x40);
        s.radio.power.clear_all_mac_power_interrupts();
        state(s, tag + 0x50);
    }
}

/// Complete sourced HAL run-before-schedule order after a stopped target crossing.
pub(in crate::image) fn recovery(s: &mut Session<'_>) {
    tbtt_recovery(s, 0..2);
}
pub(in crate::image) fn recovery23(s: &mut Session<'_>) {
    tbtt_recovery(s, 2..4);
}
fn tbtt_recovery(s: &mut Session<'_>, domains: core::ops::Range<u8>) {
    if !init(s) {
        return;
    }
    for n in domains {
        let c = Cause::Tbtt(n);
        let tag = 0x3800 + u32::from(n) * 0x100;
        load(s, n);
        if !fresh(s, c, true, tag) {
            return;
        }
        s.radio.power.clear_all_mac_power_interrupts();
        let stopped = program(s, c, tag + 0x10);
        let start = oer_riscv::cycle32();
        while oer_riscv::cycle32().wrapping_sub(start) < DEADLINE {}
        let end = snapshot(s, n, tag + 0x12);
        state(s, tag + 0x14);
        emit(tag + 0x16, u32::from(end >= stopped), raw(s), c.bit());
        // SOURCE: HAL start_station_tbtt / HW26 complete ordering, plus
        // BLOB_LIBPP_S31_TBTT_DOMAIN_CONTROLS_20261027 for domain selection.
        let target = stage(s, c, tag + 0x20, true);
        state(s, tag + 0x22);
        // Explicit project stale-event discard while every IRQ is masked.
        s.radio.power.clear_all_mac_power_interrupts();
        enable(s, c, true);
        state(s, tag + 0x24);
        let positive = wait(s, c, tag + 0x26);
        let now = snapshot(s, n, tag + 0x27);
        state(s, tag + 0x28); // No stop before either this or the policy below.
        if !positive || now < target {
            emit(
                0x31ee,
                tag + 0x20,
                u32::from(positive),
                u32::from(now >= target),
            );
            run(s, c, false);
            return;
        }
        s.radio.power.clear_all_mac_power_interrupts();
        state(s, tag + 0x30);
        let _ = wait(s, c, tag + 0x32);
        snapshot(s, n, tag + 0x33);
        state(s, tag + 0x34);
        run(s, c, false);
        enable(s, c, true);
        state(s, tag + 0x36);
        s.radio.power.clear_all_mac_power_interrupts();
        state(s, tag + 0x38);
    }
}
pub(in crate::image) fn timer_run(s: &mut Session<'_>) {
    if !init(s) {
        return;
    }
    for n in [2, 3] {
        let c = Cause::Timer(n);
        let tag = 0x3c00 + u32::from(n) * 0x100;
        load(s, n);
        let target = program(s, c, tag);
        enable(s, c, true);
        run(s, c, true);
        state(s, tag + 2);
        if !wait(s, c, tag + 4) {
            run(s, c, false);
            return;
        }
        let end = snapshot(s, n, tag + 5);
        state(s, tag + 6);
        emit(tag + 8, u32::from(end >= target), raw(s), c.bit());
        s.radio.power.clear_all_mac_power_interrupts();
        state(s, tag + 0x10);
        // Explicitly rearm run after positively witnessing its post-expiry readback.
        program(s, c, tag + 0x12);
        run(s, c, true);
        let _ = wait(s, c, tag + 0x14);
        state(s, tag + 0x16);
        enable(s, c, true);
        state(s, tag + 0x18);
        run(s, c, false);
        s.radio.power.clear_all_mac_power_interrupts();
        state(s, tag + 0x1a);
    }
}
