//! Whether hart 0 implements the unprivileged `time`/`timeh` CSRs
//! (Zicntr, 0xC01/0xC81), and their rate against `mcycle` and SYSTIMER
//! unit 0, at the inherited CPU clock (45) and with the CPU divider doubled
//! (46). An unimplemented CSR raises the probe's exception record
//! (0xef: mcause, mepc, mtval) and the 0xbad00002 marker.
use super::{Session, emit, generic};

// SAFETY: a leaf C-ABI thunk; it reads three CSRs into the caller's
// three-word buffer and changes no other state. An unimplemented CSR read
// traps into the probe's exception path, which records and halts.
core::arch::global_asm!(
    r#"
.section .text.facts_read_time,"ax"
.global facts_read_time
facts_read_time:
csrr t0, mcycle
csrr t1, time
csrr t2, timeh
sw t0, 0(a0)
sw t1, 4(a0)
sw t2, 8(a0)
ret
"#
);

unsafe extern "C" {
    fn facts_read_time(out: *mut [u32; 3]);
}

/// `[mcycle, time, timeh]` read back to back.
fn read() -> [u32; 3] {
    let mut out = [0_u32; 3];
    // SAFETY: the thunk writes exactly the three words of `out`.
    unsafe { facts_read_time(&mut out) };
    out
}

/// The 10 ms window at the 320 MHz CPU clock.
const WINDOW_CYCLES: u32 = 3_200_000;

pub(super) fn rate(s: &mut Session<'_>) {
    generic::systimer_open(s);
    emit(0x4500, 0, 0, 0);
    measure(s);
}

/// The same window with the CPU divider number one higher (the CPU clock
/// halves from XTAL 40 MHz under the ROM loader), then the inherited
/// divider restored. 0x4600: inherited root, divider and numerator;
/// 0x4601: divider applied and the update's completion; 0x46ff: the
/// restored divider and its completion.
///
/// SOURCE: production `oer_esp32s31_hal::boot::clock` (`stage`, then
/// `ROOT_CLK_CTRL0.SOC_CLK_UPDATE` until it clears; ESP-IDF 4d80c1b2
/// `clk_tree_ll.h:602-606`).
pub(super) fn rate_halved(s: &mut Session<'_>) {
    generic::systimer_open(s);
    let clocks = &s.sar.boot.hp_sys_clkrst_boot_clock;
    let root = clocks.read_soc_clk_sel().reg_soc_clk_sel().get();
    let cpu = clocks.read_cpu_freq_ctrl0();
    let inherited = cpu.reg_cpu_clk_div_num();
    emit(
        0x4600,
        root,
        u32::from(inherited),
        cpu.reg_cpu_clk_div_numerator().get(),
    );
    let Some(halved) = inherited.checked_add(1) else {
        emit(0x46ee, u32::from(inherited), 0, 0);
        return;
    };
    let applied = set_cpu_divider(s, halved);
    emit(0x4601, u32::from(halved), u32::from(applied), 0);
    if applied {
        measure(s);
    }
    let restored = set_cpu_divider(s, inherited);
    emit(0x46ff, u32::from(inherited), u32::from(restored), 0);
}

/// Stage the CPU divider number and latch it; whether the update cleared
/// within the probe's poll bound.
fn set_cpu_divider(s: &mut Session<'_>, divider: u8) -> bool {
    let clocks = &mut s.sar.boot.hp_sys_clkrst_boot_clock;
    clocks.modify_cpu_freq_ctrl0(|w| {
        w.set_reg_cpu_clk_div_num(divider);
    });
    if clocks.start_root_clk_ctrl0_reg_soc_clk_update().is_err() {
        return false;
    }
    let start = oer_riscv::cycle32();
    while clocks.root_clk_ctrl0_reg_soc_clk_update_busy() {
        if oer_riscv::cycle32().wrapping_sub(start) >= 2_000_000 {
            return false;
        }
    }
    true
}

fn measure(s: &mut Session<'_>) {
    let first = read();
    emit(0x4501, first[0], first[1], first[2]);
    let systimer_first = generic::systimer_now(s);
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < WINDOW_CYCLES {}
    let second = read();
    let systimer_second = generic::systimer_now(s);
    emit(0x4502, second[0], second[1], second[2]);
    let low = |ticks: Option<u64>| ticks.map_or(u32::MAX, |ticks| ticks as u32);
    emit(
        0x4503,
        low(systimer_first),
        low(systimer_second),
        low(systimer_second).wrapping_sub(low(systimer_first)),
    );
    emit(
        0x4504,
        second[0].wrapping_sub(first[0]),
        second[1].wrapping_sub(first[1]),
        second[2].wrapping_sub(first[2]),
    );
}
