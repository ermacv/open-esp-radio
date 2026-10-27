//! Isolated TIMG0 reload/comparator and equal-level CLIC facts (HW23).
//! No group reset, RF, DMA, watchdog configuration or production API.
use super::{CONTROLS, LINE, Session, emit, pac, setup, wait_window};
use core::{
    ptr,
    sync::atomic::{AtomicPtr, AtomicU32, Ordering},
};
use oer_mmio::{Bits, Index};

const LIMIT: u32 = 200_000;
const LOW: Index<{ pac::clic::ctrl::COUNT }> = Index::constant::<LINE>();
const HIGH: Index<{ pac::clic::ctrl::COUNT }> = Index::constant::<{ LINE + 1 }>();
static CLIC: AtomicPtr<pac::clic::Clic> = AtomicPtr::new(ptr::null_mut());
static COUNT: AtomicU32 = AtomicU32::new(0);
static CAUSES: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static CYCLES: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static LEVELS: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];

pub(super) fn interrupt() -> bool {
    let clic = CLIC.load(Ordering::Acquire);
    if clic.is_null() {
        return false;
    }
    let cycle = oer_riscv::cycle32();
    let cause = oer_riscv::read_mcause() as u32;
    // SOURCE: RISC-V CLIC specification 3f8ee744, mcause.exccode[11:0].
    // Architectural CSR decoding only; peripheral MMIO stays typed PAC.
    let id = (cause & 0xfff) as usize;
    let selected = if id == LINE {
        LOW
    } else if id == LINE + 1 {
        HIGH
    } else {
        emit(0x19ee, cause, cycle, 1);
        super::halt();
    };
    // SAFETY: arbitration publishes its live exclusive CLIC borrow while the
    // masked thread enables IRQs and waits without touching it. Both lines are
    // P2, hart1 is contained, and hardware prevents equal-level nesting. The
    // thread revokes this loan only after clearing MIE. Each entry disables its
    // own line; peripherals remain pending for post-admission observation.
    unsafe { &mut *clic }.modify_ctrl(selected, |w| {
        w.set_ie(false);
    });
    oer_riscv::fence();
    let at = COUNT.fetch_add(1, Ordering::Relaxed) as usize;
    if let (Some(c), Some(t), Some(l)) = (CAUSES.get(at), CYCLES.get(at), LEVELS.get(at)) {
        c.store(cause, Ordering::Relaxed);
        t.store(cycle, Ordering::Relaxed);
        l.store(oer_riscv::read_mintstatus() as u32, Ordering::Release);
    } else {
        emit(0x19ee, cause, cycle, 2);
        super::halt();
    }
    true
}

fn ena(s: &mut Session<'_>, value: bool) {
    s.generic.timer.timg0_timer0.modify_int_ena_timers(|w| {
        w.set_t0_int_ena(value);
    });
}
fn clear(s: &mut Session<'_>) {
    s.generic
        .timer
        .timg0_interrupt_bank
        .trigger_int_clr_timers_t0_int_clr();
    oer_riscv::fence();
}
fn raw(s: &Session<'_>) -> bool {
    s.generic
        .timer
        .timg0_interrupt_bank
        .read_int_raw_timers()
        .t0_int_raw()
}
fn stop(s: &mut Session<'_>) {
    s.generic
        .timer
        .timg0_timer0
        .configure_timg0_timer0_stopped();
}
fn load(s: &mut Session<'_>, value: u64) {
    let timer = &mut s.generic.timer.timg0_timer0;
    timer.modify_t0loadlo(|w| {
        w.set_t_load_lo(value as u32);
    });
    timer.modify_t0loadhi(|w| {
        w.set_t_load_hi(Bits::truncate((value >> 32) as u32));
    });
    timer.trigger_t0load_t_load(1);
}
fn target(s: &mut Session<'_>, value: u32) {
    let timer = &mut s.generic.timer.timg0_timer0;
    timer.modify_t0alarmlo(|w| {
        w.set_t_alarm_lo(value);
    });
    timer.modify_t0alarmhi(|w| {
        w.set_t_alarm_hi(Bits::constant::<0>());
    });
}
fn count(s: &mut Session<'_>) -> Option<u64> {
    let timer = &mut s.generic.timer.timg0_timer0;
    timer.capture_timg0_timer0();
    let start = oer_riscv::cycle32();
    while timer.read_t0update().t_update() {
        if oer_riscv::cycle32().wrapping_sub(start) >= LIMIT {
            emit(0x19ee, 0, start, 3);
            return None;
        }
    }
    Some((u64::from(timer.read_t0hi().t_hi().get()) << 32) | u64::from(timer.read_t0lo().t_lo()))
}
fn state(s: &Session<'_>, tag: u32) {
    let timer = &s.generic.timer.timg0_timer0;
    let bank = &s.generic.timer.timg0_interrupt_bank;
    let c = timer.read_t0config();
    emit(
        tag,
        u32::from(raw(s)),
        u32::from(bank.read_int_st_timers().t0_int_st()),
        u32::from(timer.read_int_ena_timers().t0_int_ena())
            | (u32::from(c.t_en()) << 1)
            | (u32::from(c.t_alarm_en()) << 2),
    );
}
fn begin(s: &mut Session<'_>) -> bool {
    setup(s);
    let wdt = &mut s.ble.watchdog;
    wdt.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Unlock.bits());
    });
    wdt.trigger_feed_feed();
    wdt.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Lock.bits());
    });
    let clock = s.generic.watchdog_clock.read_timergrp0_ctrl0();
    emit(
        0x1900,
        u32::from(clock.reg_timergrp0_apb_clk_en()),
        u32::from(clock.reg_timergrp0_rst_en()),
        clock.reg_timergrp0_t0_src_sel().get(),
    );
    if !clock.reg_timergrp0_apb_clk_en() || clock.reg_timergrp0_rst_en() {
        emit(0x19ee, 0, 0, 4);
        return false;
    }
    s.generic.watchdog_clock.modify_timergrp0_ctrl0(|w| {
        w.set_reg_timergrp0_t0_src_sel(Bits::constant::<
            { pac::hp_sys_clkrst::timergrp0_ctrl0::RegTimergrp0T0SrcSel::Xtal.bits() },
        >())
        .set_reg_timergrp0_t0_clk_en(true);
    });
    stop(s);
    ena(s, false);
    clear(s);
    s.source.store_hp_cpu_int_from_cpu_2(false);
    let root = s.sar.boot.hp_sys_clkrst_boot_clock.read_soc_clk_sel();
    let cpu = s.sar.boot.hp_sys_clkrst_boot_clock.read_cpu_freq_ctrl0();
    emit(
        0x1901,
        root.reg_soc_clk_sel().get(),
        u32::from(cpu.reg_cpu_clk_div_num()),
        cpu.reg_cpu_clk_div_numerator().get(),
    );
    let c = s.generic.timer.timg0_timer0.read_t0config();
    emit(
        0x1902,
        s.generic
            .watchdog_clock
            .read_timergrp0_ctrl0()
            .reg_timergrp0_t0_src_sel()
            .get(),
        u32::from(c.t_divider()),
        s.interrupt_matrix
            .clic_observer
            .read_int_config()
            .mnlbits()
            .get(),
    );
    true
}
fn finish(s: &mut Session<'_>) {
    stop(s);
    ena(s, false);
    clear(s);
    s.source.store_hp_cpu_int_from_cpu_2(false);
    for i in [LOW, HIGH] {
        s.interrupt_matrix.clic.modify_ctrl(i, |w| {
            w.set_ie(false);
        });
    }
    state(s, 0x19ff);
    s.generic.watchdog_clock.modify_timergrp0_ctrl0(|w| {
        w.set_reg_timergrp0_t0_clk_en(false);
    });
}
// Exact #710 arm_after transaction, with mcycle bracketing the start store.
// SOURCE: hal/src/platform/timg0_alarm.rs and ESP_IDF_4D59230D_S31_TIMER0_LL.
fn arm(s: &mut Session<'_>, us: u32) -> [u32; 2] {
    stop(s);
    ena(s, false);
    oer_riscv::fence();
    target(s, us);
    load(s, 0);
    let before = oer_riscv::cycle32();
    s.generic.timer.timg0_timer0.configure_timg0_timer0_armed();
    oer_riscv::fence();
    ena(s, true);
    oer_riscv::fence();
    [before, oer_riscv::cycle32()]
}
fn expiry(s: &Session<'_>, tag: u32, start: u32) {
    // Last negative/first positive read bracket RAW assertion plus polling
    // overhead. Neither timestamp is a CPU-interrupt latency measurement.
    let mut negative = start;
    let mut positive = false;
    let end = loop {
        let before = oer_riscv::cycle32();
        if raw(s) {
            positive = true;
            break oer_riscv::cycle32();
        }
        negative = before;
        let now = oer_riscv::cycle32();
        if now.wrapping_sub(start) >= LIMIT {
            break now;
        }
    };
    emit(
        tag,
        u32::from(positive),
        negative.wrapping_sub(start),
        end.wrapping_sub(start),
    );
}
pub(super) fn reload_rate(s: &mut Session<'_>) {
    if !begin(s) {
        return;
    }
    // A nonzero high word also checks that zero reload reaches both halves.
    let seed = (3_u64 << 32) | 12345;
    load(s, seed);
    if let Some(c) = count(s) {
        emit(0x1910, c as u32, (c >> 32) as u32, 0);
    }
    // Changing LOAD registers alone must not be mistaken for a reload.
    s.generic.timer.timg0_timer0.modify_t0loadlo(|w| {
        w.set_t_load_lo(0);
    });
    s.generic.timer.timg0_timer0.modify_t0loadhi(|w| {
        w.set_t_load_hi(Bits::constant::<0>());
    });
    if let Some(c) = count(s) {
        emit(0x1911, c as u32, (c >> 32) as u32, 0);
    }
    s.generic.timer.timg0_timer0.trigger_t0load_t_load(1);
    if let Some(c) = count(s) {
        emit(0x1912, c as u32, (c >> 32) as u32, 0);
    }
    s.generic
        .timer
        .timg0_timer0
        .configure_timg0_timer0_counting();
    if let Some(a) = count(s) {
        let start = oer_riscv::cycle32();
        while oer_riscv::cycle32().wrapping_sub(start) < 40_000 {}
        if let Some(b) = count(s) {
            emit(
                0x1913,
                a as u32,
                b as u32,
                oer_riscv::cycle32().wrapping_sub(start),
            );
        }
    }
    finish(s);
}
pub(super) fn intervals(s: &mut Session<'_>) {
    if !begin(s) {
        return;
    }
    for us in [10, 100, 1000] {
        stop(s);
        clear(s);
        load(s, (3_u64 << 32) | 12345);
        if let Some(c) = count(s) {
            emit(0x1920, us, c as u32, (c >> 32) as u32);
        }
        let [before, armed] = arm(s, us);
        expiry(s, 0x1921, before);
        emit(0x1922, us, armed.wrapping_sub(before), 0);
        if let Some(c) = count(s) {
            emit(0x1923, c as u32, (c >> 32) as u32, us);
        }
        state(s, 0x1924);
    }
    finish(s);
}
pub(super) fn pending_rearm(s: &mut Session<'_>) {
    if !begin(s) {
        return;
    }
    let [before, _] = arm(s, 10);
    expiry(s, 0x1930, before);
    state(s, 0x1931);
    let [before, armed] = arm(s, 1000); // Deliberately no CLR.
    state(s, 0x1932);
    emit(0x1933, before, armed, 1000);
    clear(s);
    state(s, 0x1934);
    expiry(s, 0x1935, before);
    if let Some(c) = count(s) {
        emit(0x1936, c as u32, (c >> 32) as u32, 0);
    }
    state(s, 0x1937);
    finish(s);
}
pub(super) fn equal_below(s: &mut Session<'_>) {
    if !begin(s) {
        return;
    }
    for alarm in [9999, 10000, 11000] {
        stop(s);
        clear(s);
        load(s, 10000);
        target(s, alarm);
        ena(s, true);
        if let Some(c) = count(s) {
            emit(0x1940, alarm, c as u32, (c >> 32) as u32);
        }
        state(s, 0x1941);
        s.generic
            .timer
            .timg0_timer0
            .configure_timg0_timer0_stopped_alarm();
        wait_window();
        state(s, 0x1942);
        let before = oer_riscv::cycle32();
        s.generic.timer.timg0_timer0.configure_timg0_timer0_armed();
        expiry(s, 0x1943, before);
        state(s, 0x1944);
    }
    finish(s);
}
pub(super) fn running_past(s: &mut Session<'_>) {
    if !begin(s) {
        return;
    }
    load(s, 10000);
    s.generic
        .timer
        .timg0_timer0
        .configure_timg0_timer0_counting();
    ena(s, true);
    clear(s);
    if let Some(c) = count(s) {
        emit(0x1950, c as u32, (c >> 32) as u32, 0);
    }
    state(s, 0x1951);
    target(s, 0);
    let before = oer_riscv::cycle32();
    s.generic.timer.timg0_timer0.configure_timg0_timer0_armed();
    expiry(s, 0x1952, before);
    state(s, 0x1953);
    finish(s);
}
fn route(s: &mut Session<'_>, source: pac::Interrupt, line: usize) {
    if let Ok(i) = Index::new(source as usize) {
        s.interrupt_matrix.interrupt_core0.modify_route(i, |w| {
            w.set_map(Bits::truncate(line as u32));
        });
    }
}
fn arbitration(s: &mut Session<'_>, timer_high: bool) {
    if !begin(s) {
        return;
    }
    let (timer, software) = if timer_high { (HIGH, LOW) } else { (LOW, HIGH) };
    route(s, pac::Interrupt::TIMERGRP0_T0, timer.get());
    route(s, pac::Interrupt::CPU_INTR_FROM_CPU_2, software.get());
    for i in [LOW, HIGH] {
        s.interrupt_matrix.clic.modify_ctrl(i, |w| {
            w.set_attr_shv(true)
                .set_attr_trig(pac::clic::ctrl::AttrTrig::PositiveLevel)
                .set_ctl(CONTROLS[1])
                .set_ie(true);
        });
    }
    let [before, _] = arm(s, 10);
    expiry(s, 0x1960, before);
    s.source.store_hp_cpu_int_from_cpu_2(true);
    wait_window();
    emit(
        0x1961,
        timer.get() as u32,
        software.get() as u32,
        u32::from(CONTROLS[1]),
    );
    state(s, 0x1962);
    emit(
        0x1963,
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_0()
            .int_status_0(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_2()
            .int_status_2(),
        u32::from(
            s.source
                .read_hp_cpu_int_from_cpu_2()
                .hp_cpu_int_from_cpu_2(),
        ),
    );
    for i in [LOW, HIGH] {
        let c = s.interrupt_matrix.clic.read_ctrl(i);
        emit(
            0x1964,
            i.get() as u32,
            u32::from(c.ip()) | (u32::from(c.ie()) << 1),
            u32::from(c.ctl()),
        );
    }
    COUNT.store(0, Ordering::Relaxed);
    for a in CAUSES.iter().chain(&CYCLES).chain(&LEVELS) {
        a.store(0, Ordering::Relaxed);
    }
    CLIC.store(
        ptr::addr_of_mut!(s.interrupt_matrix.clic),
        Ordering::Release,
    );
    let start = oer_riscv::cycle32();
    oer_riscv::enable_interrupts();
    wait_window();
    oer_riscv::disable_interrupts();
    CLIC.store(ptr::null_mut(), Ordering::Release);
    emit(
        0x1965,
        COUNT.load(Ordering::Acquire),
        start,
        oer_riscv::cycle32(),
    );
    for ((c, t), l) in CAUSES.iter().zip(&CYCLES).zip(&LEVELS) {
        emit(
            0x1966,
            c.load(Ordering::Relaxed),
            t.load(Ordering::Relaxed).wrapping_sub(start),
            l.load(Ordering::Relaxed),
        );
    }
    state(s, 0x1967);
    route(s, pac::Interrupt::TIMERGRP0_T0, 0);
    route(s, pac::Interrupt::CPU_INTR_FROM_CPU_2, 0);
    finish(s);
}
pub(super) fn timer_low(s: &mut Session<'_>) {
    arbitration(s, false);
}
pub(super) fn timer_high(s: &mut Session<'_>) {
    arbitration(s, true);
}
