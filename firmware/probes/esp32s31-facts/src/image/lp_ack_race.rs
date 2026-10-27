//! Finite arrival orderings; no simultaneous set/clear priority claim.
use super::{Session, ble, emit};

const MAX: u32 = super::pac::modem_pwr::word_0048_opaque::COUNTER_LOW_MAX;
const SHORT: u32 = 200_000;
const WRAP: u32 = 40_000_000;

fn counter(s: &Session<'_>) -> u32 {
    s.ble.timer.read_word_0048_opaque().counter_low().get()
}
fn latch(s: &Session<'_>) -> bool {
    s.ble.timer.read_word_0058_opaque().control_2()
}
fn byte(s: &Session<'_>) -> u8 {
    s.ble.timer.read_word_0024_opaque().low_byte()
}
fn window() {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < 4096 {}
}
fn state(s: &Session<'_>, tag: u32) {
    emit(
        tag,
        counter(s),
        s.ble
            .timer_status
            .read_word_0038_opaque()
            .bits_0_31_opaque(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_3()
            .int_status_3(),
    );
    emit(
        tag + 1,
        byte(s).into(),
        s.ble.timer.read_word_002c_opaque().low_byte().into(),
        u32::from(latch(s)),
    );
}
fn park(s: &mut Session<'_>) {
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    // SOURCE: HW16_S31_LP_MODE58_20261026 / HW21_S31_LP_CONTROL4_20261027.
    // CONTROL2 is an independent compare witness while byte24 is already1.
    s.ble.timer.modify_word_0058_opaque(|w| {
        w.set_control_0(true).set_control_4(false);
    });
    s.ble.timer.trigger_word_0058_opaque_control_1();
    oer_riscv::fence();
}
fn init(s: &mut Session<'_>) {
    oer_riscv::disable_interrupts();
    ble::init_lp(s);
    let cpu = s.sar.boot.hp_sys_clkrst_boot_clock.read_cpu_freq_ctrl0();
    let pll = s.sar.boot.lp_aon_clkrst_cpll.read_cpll_div();
    emit(
        0x180f0,
        s.sar
            .boot
            .hp_sys_clkrst_boot_clock
            .read_soc_clk_sel()
            .reg_soc_clk_sel()
            .get(),
        cpu.reg_cpu_clk_div_num().into(),
        cpu.reg_cpu_clk_div_numerator().get(),
    );
    emit(
        0x180f1,
        pll.cpll_fb_div().into(),
        pll.cpll_ref_div().get(),
        MAX,
    );
    park(s);
}
fn stop(s: &mut Session<'_>) {
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_002c();
    oer_riscv::fence();
}
fn arm(s: &mut Session<'_>, target: u32, tag: u32) {
    s.ble.timer.modify_word_006c_opaque(|w| {
        w.set_bits_0_31_opaque(target);
    });
    let before = counter(s);
    s.ble.timer.write_word_0010_opaque(|w| {
        w.set_bit_18_opaque(true);
    });
    oer_riscv::fence();
    emit(tag, target, before, counter(s));
}
fn wait_latch(s: &Session<'_>) -> bool {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < SHORT {
        if latch(s) {
            return true;
        }
    }
    false
}

pub(super) fn compare(s: &mut Session<'_>) {
    init(s);
    // Three different causal orderings, once each. Software seeds a known
    // existing byte in the nonzero cases; CONTROL2 separately witnesses the
    // subsequent hardware compare. No output records between sample and ack.
    for phase in 0..3 {
        let tag = 0x18000 + phase * 0x100;
        park(s);
        if phase != 0 {
            s.ble
                .timer
                .publish_bluetooth_modem_lp_timer_software_pending();
        }
        state(s, tag);
        let target = counter(s).wrapping_add(2048) & MAX;
        arm(s, target, tag + 2);
        let sampled_at = counter(s);
        let old = byte(s);
        let latch_at_sample = latch(s);
        let arrived = phase != 2 && wait_latch(s);
        let before = counter(s);
        let stamp = oer_riscv::cycle32();
        if old != 0 {
            s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
        }
        oer_riscv::fence();
        let elapsed = oer_riscv::cycle32().wrapping_sub(stamp);
        let after = counter(s);
        emit(tag + 3, old.into(), sampled_at, u32::from(latch_at_sample));
        emit(tag + 4, u32::from(arrived), before, after);
        emit(tag + 5, elapsed, u32::from(old != 0), target);
        state(s, tag + 0x10);
        let eventual = wait_latch(s);
        window();
        emit(tag + 6, u32::from(eventual), 0, 0);
        state(s, tag + 0x20);
    }
    stop(s);
}

pub(super) fn wraps(s: &mut Session<'_>) {
    init(s);
    // Prior measured wraps need ~33.55M cycles each; two are a sub-second
    // observation at the inherited 80MHz, capped at 80M cycles altogether.
    for phase in 0..2 {
        let old = s.ble.timer.read_word_002c_opaque().low_byte();
        let start = oer_riscv::cycle32();
        let mut prior = counter(s);
        let mut wrapped = false;
        let mut now = prior;
        while oer_riscv::cycle32().wrapping_sub(start) < WRAP {
            now = counter(s);
            if now < prior {
                wrapped = true;
                break;
            }
            prior = now;
        }
        let elapsed = oer_riscv::cycle32().wrapping_sub(start);
        state(s, 0x18300 + phase * 0x10);
        // Actual nonzero conditional path, with the second natural crossing
        // after its retained first-wrap sample. No ack between the wraps.
        if phase == 1 && old != 0 {
            s.ble.timer.clear_bluetooth_modem_lp_timer_state_002c();
        }
        oer_riscv::fence();
        emit(0x18340 + phase, old.into(), u32::from(wrapped), elapsed);
        emit(0x18350 + phase, prior, now, counter(s));
        if !wrapped {
            stop(s);
            return;
        }
    }
    state(s, 0x18320);
    window();
    state(s, 0x18330);
    stop(s);
}

pub(super) fn rearm(s: &mut Session<'_>) {
    init(s);
    for (phase, offset) in [(0, 2048_u32), (1, 64), (2, MAX - 63)] {
        let tag = 0x18400 + phase * 0x100;
        park(s);
        // A positive software cause is actually sampled and conditionally
        // acknowledged before each fresh target. No clear after publication.
        s.ble
            .timer
            .publish_bluetooth_modem_lp_timer_software_pending();
        let old = byte(s);
        if old != 0 {
            s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
        }
        oer_riscv::fence();
        state(s, tag);
        let origin = counter(s);
        let target = origin.wrapping_add(offset) & MAX;
        arm(s, target, tag + 2);
        let start = oer_riscv::cycle32();
        let mut crossed = false;
        while oer_riscv::cycle32().wrapping_sub(start) < SHORT {
            if counter(s).wrapping_sub(origin) & MAX >= 4096 {
                crossed = true;
                break;
            }
        }
        emit(tag + 3, old.into(), origin, offset);
        emit(
            tag + 4,
            u32::from(crossed),
            counter(s),
            oer_riscv::cycle32().wrapping_sub(start),
        );
        state(s, tag + 0x10);
    }
    // Recover from the deliberately past target with a fresh future compare.
    park(s);
    state(s, 0x18700);
    let target = counter(s).wrapping_add(2048) & MAX;
    arm(s, target, 0x18702);
    let arrived = wait_latch(s);
    emit(0x18703, u32::from(arrived), counter(s), 0);
    state(s, 0x18710);
    stop(s);
}
