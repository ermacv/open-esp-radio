//! HP system reset with positive BLE LP causes, followed by read-before-prepare.
use super::{
    Bits, FACTS_OUTPUT, Ordering, RETAINED, Session, ble, emit, halt, pac, retained_record,
};

const MAGIC: u32 = 0xfac70007;
const REASON: u32 = 0x687713;
const LOW_MAX: u32 = pac::modem_pwr::word_0048_opaque::COUNTER_LOW_MAX;
// One natural rollover at the existing XTAL/divider-1 setup, never a series.
const LIMIT: u32 = 40_000_000;

pub(super) fn is_record(r: &[u32; 12]) -> bool {
    r[0] == MAGIC && r[1] == REASON && r[2] <= 3 && r[11] == !r[2]
}
fn counter(s: &Session<'_>) -> u32 {
    s.ble.timer.read_word_0048_opaque().counter_low().get()
}
// The host retains the whole matrix word; source identity/geometry comes from PAC.
fn matrix(s: &Session<'_>) -> u32 {
    s.interrupt_matrix
        .interrupt_core0_status
        .read_int_status_reg_3()
        .int_status_3()
}
fn snapshot(s: &Session<'_>) -> [u32; 5] {
    let output = matrix(s);
    let t = &s.ble.timer;
    [
        t.read_word_0038_opaque().bits_0_31_opaque(),
        u32::from(t.read_word_0024_opaque().low_byte()),
        u32::from(t.read_word_002c_opaque().low_byte()),
        counter(s),
        output,
    ]
}
fn state(s: &Session<'_>, tag: u32) -> [u32; 5] {
    let v = snapshot(s);
    emit(tag, v[0], v[1], v[2]);
    emit(tag + 1, v[3], v[4], oer_riscv::cycle32());
    v
}
fn clocks(s: &Session<'_>, tag: u32) {
    let m = s.ble.upstream.read_modem_conf();
    let c = s.ble.gates.read_clk_conf();
    let l = s.ble.lp_clock.read_lp_timer_conf();
    emit(
        tag,
        u32::from(m.modem_apb_clk_en()),
        u32::from(c.clk_wifipwr_en()),
        u32::from(c.clk_lp_timer_en()),
    );
    emit(
        tag + 1,
        u32::from(l.clk_lp_timer_sel_xtal()),
        l.clk_lp_timer_div_num().get(),
        u32::from(s.ble.clocks.read_clk_conf().clk_ble_timer_en()),
    );
}
fn window() {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < 4096 {}
}
fn wait_wrap(s: &Session<'_>, tag: u32) -> bool {
    let start = oer_riscv::cycle32();
    let mut prior = counter(s);
    while oer_riscv::cycle32().wrapping_sub(start) < LIMIT {
        let now = counter(s);
        if now < prior {
            emit(tag, prior, now, oer_riscv::cycle32().wrapping_sub(start));
            return true;
        }
        prior = now;
    }
    emit(tag, prior, counter(s), 0);
    false
}
fn enable(s: &mut Session<'_>, compare: bool, rollover: bool) {
    s.ble.timer.write_word_0010_opaque(|w| {
        w.set_bit_18_opaque(compare).set_bit_1_opaque(rollover);
    });
    oer_riscv::fence();
}
fn clear(s: &mut Session<'_>) {
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_002c();
    oer_riscv::fence();
}
fn run(s: &mut Session<'_>, scenario: u32) {
    ble::init_lp(s);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    if (scenario == 1 || scenario == 2) && !wait_wrap(s, 0xb000) {
        return;
    }
    let compare = scenario == 0 || scenario == 2;
    let target = counter(s).wrapping_add(if compare { 4096 } else { LOW_MAX / 2 }) & LOW_MAX;
    s.ble.timer.modify_word_006c_opaque(|w| {
        w.set_bits_0_31_opaque(target);
    });
    enable(s, true, true);
    if compare {
        let start = oer_riscv::cycle32();
        while s.ble.timer.read_word_0024_opaque().low_byte() == 0
            && oer_riscv::cycle32().wrapping_sub(start) < 200_000
        {}
    }
    let before = state(s, 0xb010);
    let expected24 = u32::from(compare);
    let expected2c = u32::from(scenario == 1 || scenario == 2);
    emit(0xb012, scenario, expected24, expected2c);
    if before[1] != expected24 || before[2] != expected2c {
        return;
    }

    // SOURCE: staged-runtime fatal::reset_recording -> HAL cores::{fatal,
    // stop_other, software_reset, prepare_reset}, hart0, other hart already Held.
    // No HAL call, peripheral takeover, UART perturbation or timer cleanup.
    let _ = oer_riscv::swap_interrupt_threshold(u8::MAX);
    oer_riscv::disable_interrupts();
    core::sync::atomic::compiler_fence(Ordering::SeqCst);
    let held = s.core_control.hp_sys_clkrst_core.read_hpcore1_ctrl0();
    if !held.reg_core1_global_rst_en() || held.reg_core1_cpu_clk_en() {
        return;
    }
    // The recording closure samples only. The register sequence starts here.
    let before = snapshot(s);
    s.uart0_clock.hp_sys_clkrst_uart0.modify_uart0_ctrl0(|w| {
        w.set_reg_uart0_clk_src_sel(Bits::constant::<0>())
            .set_reg_uart0_sclk_div_num(0)
            .set_reg_uart0_sclk_div_numerator(0)
            .set_reg_uart0_sclk_div_denominator(0)
            .set_reg_uart0_clk_en(true);
    });
    s.core_control
        .lp_sys_core_control
        .modify_boot_addr_hp_core1(|w| {
            w.set_boot_addr_hp_core1(0);
        });
    s.core_control
        .lp_aon_clkrst_core_reset
        .trigger_hpcore1_reset_ctrl_hpcore1_sw_reset();
    s.core_control.pmu_cpu_stall.modify_cpu_stall_sw(|w| {
        w.set_hpcore1_sw_stall_code(0x86);
    });
    RETAINED[0].store(0, Ordering::Relaxed);
    for (word, value) in RETAINED.iter().skip(1).zip([
        REASON,
        scenario,
        before[0],
        before[1],
        before[2],
        before[3],
        before[4],
        target,
        oer_riscv::cycle32(),
        1,
        !scenario,
    ]) {
        word.store(value, Ordering::Relaxed);
    }
    RETAINED[0].store(MAGIC, Ordering::Release);
    FACTS_OUTPUT[0].store(MAGIC, Ordering::Release);
    oer_riscv::fence();
    s.core_control
        .lp_sys_core_control
        .trigger_sys_ctrl_sys_sw_rst();
    halt()
}
pub(super) fn compare(s: &mut Session<'_>) {
    run(s, 0);
}
pub(super) fn rollover(s: &mut Session<'_>) {
    run(s, 1);
}
pub(super) fn both(s: &mut Session<'_>) {
    run(s, 2);
}
pub(super) fn control(s: &mut Session<'_>) {
    run(s, 3);
}

pub(super) fn observe(s: &mut Session<'_>) {
    // First hardware reads of this boot: no contain, clocks, prepare, reset,
    // enable, acknowledge or timer start precedes this matrix/LP snapshot.
    state(s, 0xb100);
    clocks(s, 0xb102);
    let r = retained_record();
    for (i, row) in r.chunks_exact(3).enumerate() {
        if let [a, b, c] = row {
            emit(0xb110 + i as u32, *a, *b, *c);
        }
    }
    emit(
        0xb114,
        s.reset_cause
            .lp_aon_reset_cause
            .read_hpcore0_reset_cause()
            .hpcore0_reset_cause()
            .get(),
        s.reset_cause
            .lp_aon_reset_cause
            .read_hpcore1_reset_cause()
            .hpcore1_reset_cause()
            .get(),
        u32::from(is_record(&r)),
    );
    RETAINED[0].store(0, Ordering::Release);
    if !is_record(&r) {
        return;
    }
    // Diagnostic phase: restore access clocks only. No LP prepare, reset,
    // pending clear, counter start or +0x10 enable command before this sample.
    ble::clocks(&mut s.ble);
    state(s, 0xb120);
    window();
    state(s, 0xb122);
    // +0x10 is write-only. Fresh software work tests its bit18 lane without
    // re-enabling it; an explicit enable afterward is the positive control.
    clear(s);
    s.ble
        .timer
        .publish_bluetooth_modem_lp_timer_software_pending();
    oer_riscv::fence();
    window();
    state(s, 0xb130);
    enable(s, true, false);
    window();
    state(s, 0xb132);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    clear(s);
    // Separate rollover diagnostic: establish counter clocks/start, without
    // bit1 enable. One natural wrap tests retained delivery, then enable it.
    s.ble.lp_clock.modify_lp_timer_conf(|w| {
        w.set_clk_lp_timer_sel_osc_slow(false)
            .set_clk_lp_timer_sel_osc_fast(false)
            .set_clk_lp_timer_sel_xtal32k(false)
            .set_clk_lp_timer_sel_xtal(true)
            .set_clk_lp_timer_div_num(Bits::constant::<1>());
    });
    s.ble.timer.write_runtime_timer_start_command(
        pac::modem_pwr::runtime_timer_start_command::Command::Start,
        |_| {},
    );
    if wait_wrap(s, 0xb140) {
        state(s, 0xb142);
        enable(s, false, true);
        window();
        state(s, 0xb144);
    }
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    clear(s);
}
