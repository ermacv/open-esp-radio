//! Selections 232/233: hart 1's state when the rev0 ROM hands it to us.
//!
//! Hart 0 records the boot inputs the ROM consults, parks every hart-1
//! matrix route, releases hart 1 in ESP-IDF's `start_other_core` order and
//! waits for a bounded window. The assembly entry (`hart1-entry.s`) clears
//! MIE with its first instruction, installs a recording trap stub and stores
//! the CSRs before anything else; the Rust part below reads hart 1's own
//! CLIC view through the session's typed CLIC owners while hart 0 waits.
//! Hart 0 then parks hart 1 again and emits the words.
//!
//! 232 writes the boot address after release (the ROM `main` path, bit 31
//! clear); 233 writes it with bit 31 set before release, which the rev0 ROM
//! `normal_boot` takes as a direct jump that skips `main` and its PMA setup.
//! SOURCE: ESP-IDF 4d80c1b2 `components/esp_system/port/cpu_start.c:305-345`
//! (`start_other_core`), `components/hal/esp32s31/include/hal/cpu_utility_ll.h`
//! (unstall code 0xFF and stall-status wait; CPU and CLIC clock enables, then
//! global reset release); rev0 ROM `.init.text` 0x2f8017d8-0x2f8017ee (bit 31).
use super::{Bits, Index, Ordering, SECONDARY, Session, emit, pac};
use core::{
    ptr,
    sync::atomic::{AtomicPtr, AtomicU32},
};

/// Hart 1's record. Word 0 is its stage marker (`0xfac71001` after the CSR
/// snapshot, `0xfac71002` after the CLIC view); 124..=127 is the trap stub's
/// marker, mcause, mepc and mtval.
// SAFETY: unique exported buffer; hart 1 writes it only between release and
// park, while hart 0 only reads it; hart 0 clears it before each release.
#[unsafe(no_mangle)]
static FACTS_HART1: [AtomicU32; 128] = [const { AtomicU32::new(0) }; 128];
static CLIC: AtomicPtr<pac::clic::Clic> = AtomicPtr::new(ptr::null_mut());
static OBSERVER: AtomicPtr<pac::clic::ClicObserver> = AtomicPtr::new(ptr::null_mut());
static PENDING: AtomicPtr<pac::clic::ClicPending<1>> = AtomicPtr::new(ptr::null_mut());
/// What hart 1 runs once released: `SNAPSHOT` (232, 233) or `CROSS` (323).
static HART1_MODE: AtomicU32 = AtomicU32::new(SNAPSHOT);
/// 323's lockstep: hart 0 names the phase, hart 1 acknowledges it.
static PHASE: AtomicU32 = AtomicU32::new(0);
static ACK: AtomicU32 = AtomicU32::new(0);

const CLIC_DONE: u32 = 0xfac7_1002;
/// 323's end on hart 1, and its refusal of a hart that is not hart 1.
const CROSS_READY: u32 = 0xfac7_1004;
const CROSS_DONE: u32 = 0xfac7_1003;
const CROSS_NOT_HART1: u32 = 0xfac7_100e;
const SNAPSHOT: u32 = 0;
const CROSS: u32 = 1;
/// 323's line: the application grid's task line, disabled on both harts.
const CROSS_LINE: usize = 34;
/// 323's trials each way.
const CROSS_TRIALS: u32 = 100;
/// 323's words in `FACTS_HART1`, written by hart 1 past the entry's CSR
/// snapshot (words 1 to 68), in the CLIC view's words, which 323 does not
/// take: its reads of its own byte while hart 0's is pended (clear), after
/// its own store (set) and after its own clear (clear), and its line's
/// enable and trigger before and after its setup.
const CROSS_FOREIGN_CLEAR: usize = 100;
const CROSS_OWN_SET: usize = 101;
const CROSS_OWN_CLEARED: usize = 102;
const CROSS_LINE_STATE: usize = 103;
const _: () = if CROSS_LINE_STATE >= TRAP {
    panic!("323's words lie below the trap words");
};
/// A phase's bound, in cycles: far above one handshake.
const PHASE_CYCLES: u32 = 1_000_000;
const TRAPPED: u32 = 0xfac7_100f;
const INFO: usize = 70;
const CONFIG: usize = 71;
const THRESHOLD: usize = 72;
const LINES: usize = 73;
const TRAP: usize = 124;
const _: () = if LINES + pac::clic::ip::COUNT > TRAP {
    panic!("CLIC lines overlap the trap words");
};
/// Release wait and stall-status polls, both far above the expected path.
const WAIT_CYCLES: u32 = 40_000_000;
const STALL_POLLS: u32 = 100_000;

// SAFETY: the entry/trap stubs only store to FACTS_HART1 and read CSRs; the
// entry installs gp and the reserved hart-1 stack before its only call.
core::arch::global_asm!(
    include_str!("../hart1-entry.s"),
    mstatus = const oer_portable_riscv::machine::MSTATUS,
    mie = const oer_portable_riscv::machine::status::MIE,
    mtvec = const oer_portable_riscv::machine::MTVEC,
    mscratch = const oer_portable_riscv::machine::MSCRATCH,
    mepc = const oer_portable_riscv::machine::MEPC,
    mcause = const oer_portable_riscv::machine::MCAUSE,
    mtvt = const oer_portable_riscv::clic::MTVT,
    mintthresh = const oer_portable_riscv::clic::MINTTHRESH,
    mintstatus = const oer_portable_riscv::clic::MINTSTATUS,
);

// SAFETY: address only; the assembly above defines the entry.
unsafe extern "C" {
    safe static facts_hart1_entry: [u8; 0];
}

/// Hart 1's Rust part: the CLIC as hart 1 addresses it. Hart 0 published the
/// session's CLIC owners and touches neither until hart 1 is parked again.
#[unsafe(no_mangle)]
extern "C" fn facts_hart1_main() -> ! {
    let clic = CLIC.load(Ordering::Acquire);
    let observer = OBSERVER.load(Ordering::Acquire);
    let pending = PENDING.load(Ordering::Acquire);
    if !clic.is_null() && !observer.is_null() && !pending.is_null() {
        // SAFETY: hart 0 lent these owners for this window, derived from
        // its exclusive borrow of the session, and only polls FACTS_HART1
        // (and, in 323, the phase words) until hart 1 is parked; it never
        // touches hart 1's CLIC parts meanwhile, which only hart 1 reaches.
        // Every access below is a read, but for 323's setup of hart 1's own
        // line 34 (its enable and trigger, through the exclusive CLIC part)
        // and its stores of hart 1's own pending byte through the shared
        // part (`&self`, hart 1's proof), which hart 0 never stores.
        let (clic, observer, pending) = unsafe { (&mut *clic, &*observer, &*pending) };
        if HART1_MODE.load(Ordering::Acquire) == CROSS {
            cross(clic, pending);
        } else {
            snapshot(clic, observer);
        }
    }
    loop {
        oer_portable_riscv::wait_for_interrupt();
    }
}

/// 323 on hart 1: answer each of hart 0's phases on hart 1's own byte of
/// `CROSS_LINE`. Phase `3k + 1`: hart 0 stored its own byte pended; hart 1
/// reads its own. `3k + 2`: hart 1 stores its own pended and reads it.
/// `3k + 3`: hart 1 stores its own cleared and reads it.
fn cross(clic: &mut pac::clic::Clic, pending: &pac::clic::ClicPending<1>) {
    let (Ok(hart), Ok(i)) = (
        oer_portable_riscv::OnHart::<1>::check(),
        Index::new(CROSS_LINE),
    ) else {
        store(0, CROSS_NOT_HART1);
        return;
    };
    let before = line_state(clic, i);
    edge_disabled(clic, i);
    store(CROSS_LINE_STATE, before | line_state(clic, i) << 8);
    if let Some(word) = FACTS_HART1.first() {
        word.store(CROSS_READY, Ordering::Release);
    }
    let mut seen = 0u32;
    while seen < 3 * CROSS_TRIALS {
        let start = oer_portable_riscv::cycle32();
        let mut phase = seen;
        while phase == seen && oer_portable_riscv::cycle32().wrapping_sub(start) < PHASE_CYCLES {
            phase = PHASE.load(Ordering::Acquire);
        }
        if phase == seen {
            break;
        }
        let (slot, held) = match phase % 3 {
            1 => (CROSS_FOREIGN_CLEAR, !clic.read_ip(i).ip()),
            2 => {
                pending.pend_line(i, &hart);
                (CROSS_OWN_SET, clic.read_ip(i).ip())
            }
            _ => {
                pending.clear_line(i, &hart);
                (CROSS_OWN_CLEARED, !clic.read_ip(i).ip())
            }
        };
        if held {
            store(slot, load(slot).wrapping_add(1));
        }
        ACK.store(phase, Ordering::Release);
        seen = phase;
    }
    if let Some(word) = FACTS_HART1.first() {
        word.store(CROSS_DONE, Ordering::Release);
    }
}

/// A line's enable (bit 0) and trigger (bits 1 to 2).
fn line_state(clic: &pac::clic::Clic, i: Index<{ pac::clic::ip::COUNT }>) -> u32 {
    u32::from(clic.read_ie(i).ie()) | clic.read_attr(i).trig().bits() << 1
}

/// Leave the line disabled and positive-edge, the one trigger whose pending
/// bit is software's: a level line's bit follows its source.
fn edge_disabled(clic: &mut pac::clic::Clic, i: Index<{ pac::clic::ip::COUNT }>) {
    clic.modify_ie(i, |w| {
        w.set_ie(false);
    });
    clic.modify_attr(i, |w| {
        w.set_trig(pac::clic::attr::Trig::PositiveEdge);
    });
}

/// 232 and 233 on hart 1: hart 1's CLIC as it addresses it.
fn snapshot(clic: &pac::clic::Clic, observer: &pac::clic::ClicObserver) {
    {
        let info = clic.read_int_info();
        store(
            INFO,
            info.num_int().get() | (info.ctlbits().get() << 16) | (u32::from(info.version()) << 24),
        );
        let config = observer.read_int_config();
        store(
            CONFIG,
            config.mnlbits().get()
                | (u32::from(config.nvbits()) << 4)
                | (config.nmbits().get() << 8),
        );
        store(
            THRESHOLD,
            u32::from(observer.read_int_thresh().cpu_int_thresh()),
        );
        for n in 0..pac::clic::ip::COUNT {
            let Ok(i) = Index::new(n) else { continue };
            let attributes = clic.read_attr(i);
            store(
                LINES + n,
                u32::from(clic.read_ctl(i).ctl())
                    | (attributes.mode().get() << 8)
                    | (u32::from(clic.read_ip(i).ip()) << 16)
                    | (u32::from(clic.read_ie(i).ie()) << 17)
                    | (u32::from(attributes.shv()) << 18)
                    | (attributes.trig().bits() << 19),
            );
        }
    }
    if let Some(word) = FACTS_HART1.first() {
        word.store(CLIC_DONE, Ordering::Release);
    }
}

fn store(index: usize, value: u32) {
    if let Some(word) = FACTS_HART1.get(index) {
        word.store(value, Ordering::Relaxed);
    }
}

fn load(index: usize) -> u32 {
    FACTS_HART1
        .get(index)
        .map_or(0, |word| word.load(Ordering::Relaxed))
}

fn feed(s: &mut Session<'_>) {
    let wdt = &mut s.watchdog;
    wdt.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Unlock.bits());
    });
    wdt.trigger_feed_feed();
    wdt.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Lock.bits());
    });
}

/// The ROM's two `ets_printf` output slots, `_putc1` and `_putc2`.
/// SOURCE: pinned rev0 ROM symbols (Blobray `symbols`): `.bss_ets_printf`
/// words 0x2f07fa28 and 0x2f07fa24, written only by `ets_install_putc1/2`.
fn rom_printf_slots() -> [u32; 2] {
    [0x2f07_fa28usize, 0x2f07_fa24].map(|address| {
        // SAFETY: aligned ROM-owned words in internal SRAM above the probe's
        // loader region; reading has no side effect and nothing writes them now.
        unsafe { ptr::read_volatile(ptr::with_exposed_provenance::<u32>(address)) }
    })
}

fn boot_inputs(s: &Session<'_>, tag: u32) {
    let c = &s.core_control;
    let cause = |hart: usize| {
        Index::new(hart).map_or(u32::MAX, |i| {
            let r = s.reset_cause.lp_aon_reset_cause.read_hpcore_reset_cause(i);
            r.hpcore_reset_cause().get() | (u32::from(r.hpcore_reset_flag()) << 8)
        })
    };
    let control = c
        .hp_sys_clkrst_core
        .read_hpcore_ctrl0(SECONDARY.hart().index());
    let status = c.hp_sys_core_stall_status.read_hp_cpu_corestalled_st();
    let icache = s.sar.boot.cache_boot.read_l1_icache_ctrl();
    let [putc1, putc2] = rom_printf_slots();
    emit(
        tag,
        cause(0),
        cause(1),
        u32::from(c.lp_sys_core_control.read_sys_ctrl().lp_core_disable()),
    );
    emit(
        tag + 1,
        s.secondary_launch
            .lp_wake_stub
            .read_lp_store8()
            .lp_scratch8(),
        c.lp_sys_core_control
            .read_boot_addr_hp_core(SECONDARY.index())
            .boot_addr_hp_core(),
        u32::from(control.reg_core_cpu_clk_en())
            | (u32::from(control.reg_core_clic_clk_en()) << 1)
            | (u32::from(control.reg_core_global_rst_en()) << 2)
            | (u32::from(control.reg_core_force_norst()) << 3)
            | (u32::from(status.hp_reg_core_corestalled_st(SECONDARY.hart().index())) << 4)
            | (u32::from(
                c.pmu_cpu_stall
                    .read_cpu_stall_sw()
                    .hpcore_sw_stall_code(SECONDARY.hart().index()),
            ) << 8),
    );
    emit(
        tag + 2,
        u32::from(icache.l1_icache_shut_ibus0()) | (u32::from(icache.l1_icache_shut_ibus1()) << 1),
        putc1,
        putc2,
    );
}

/// Disconnect every hart-1 matrix route; returns (nonzero routes, first).
fn park_routes(s: &mut Session<'_>) -> (u32, u32) {
    let bank = &mut s.secondary_launch.interrupt_core1;
    let mut routed = 0u32;
    let mut first = u32::MAX;
    for n in 0..pac::interrupt_core::route::COUNT {
        let Ok(i) = Index::new(n) else { continue };
        if bank.read_route(i).map().get() != 0 {
            routed = routed.saturating_add(1);
            if first == u32::MAX {
                first = n as u32;
            }
            bank.modify_route(i, |w| {
                w.set_map(Bits::constant::<0>());
            });
        }
    }
    oer_portable_riscv::fence();
    (routed, first)
}

fn set_boot_address(s: &mut Session<'_>, value: u32) {
    s.core_control
        .lp_sys_core_control
        .modify_boot_addr_hp_core(SECONDARY.index(), |w| {
            w.set_boot_addr_hp_core(value);
        });
    oer_portable_riscv::fence();
}

/// Park hart 1 as the session found it: boot word zero, stall requested,
/// clocks gated and global reset held.
fn park(s: &mut Session<'_>) {
    set_boot_address(s, 0);
    super::contain(s);
    oer_portable_riscv::fence();
}

pub(super) fn main_path(s: &mut Session<'_>) {
    run(s, Path::Normal);
}

pub(super) fn direct_path(s: &mut Session<'_>) {
    run(s, Path::Direct);
}

/// Whether hart 1 starts by the ROM's direct jump or the normal boot path.
#[derive(Clone, Copy, PartialEq)]
enum Path {
    Normal,
    Direct,
}

fn run(s: &mut Session<'_>, direct: Path) {
    let tag = if direct == Path::Direct {
        0x2330
    } else {
        0x2320
    };
    feed(s);
    park(s);
    boot_inputs(s, tag);
    let (routed, first) = park_routes(s);
    lend(s, SNAPSHOT);
    let (entry, polls, start) = release(s, direct);
    let mut stage = 0;
    let mut elapsed = 0;
    while elapsed < WAIT_CYCLES {
        stage = FACTS_HART1
            .first()
            .map_or(0, |word| word.load(Ordering::Acquire));
        let trapped = FACTS_HART1
            .get(TRAP)
            .is_some_and(|word| word.load(Ordering::Acquire) == TRAPPED);
        elapsed = oer_portable_riscv::cycle32().wrapping_sub(start);
        if stage == CLIC_DONE || trapped {
            break;
        }
    }
    park(s);
    revoke();
    emit(tag + 3, routed, first, polls);
    emit(tag + 4, stage, elapsed, entry);
    // Hart 1's words in triples: tag + 0x100 + first index.
    for first in (1..FACTS_HART1.len()).step_by(3) {
        emit(
            tag + 0x100 + first as u32,
            load(first),
            load(first + 1),
            load(first + 2),
        );
    }
    boot_inputs(s, tag + 5);
    feed(s);
}

/// Clear hart 1's record and lend it hart 1's CLIC parts for `mode`.
fn lend(s: &mut Session<'_>, mode: u32) {
    for word in &FACTS_HART1 {
        word.store(0, Ordering::Relaxed);
    }
    PHASE.store(0, Ordering::Relaxed);
    ACK.store(0, Ordering::Relaxed);
    HART1_MODE.store(mode, Ordering::Relaxed);
    CLIC.store(
        ptr::from_mut(&mut s.interrupt_matrix.clic[1]),
        Ordering::Relaxed,
    );
    OBSERVER.store(
        ptr::from_ref(&s.interrupt_matrix.clic_observer[1]).cast_mut(),
        Ordering::Relaxed,
    );
    PENDING.store(
        ptr::from_ref(&s.interrupt_matrix.clic_pending.1).cast_mut(),
        Ordering::Relaxed,
    );
    oer_portable_riscv::fence();
}

/// Take hart 1's loan back, once it is parked.
fn revoke() {
    CLIC.store(ptr::null_mut(), Ordering::Relaxed);
    OBSERVER.store(ptr::null_mut(), Ordering::Relaxed);
    PENDING.store(ptr::null_mut(), Ordering::Relaxed);
}

/// Release hart 1 into the probe's entry in ESP-IDF's order: the entry's
/// address, the stall polls and the cycle of the release.
fn release(s: &mut Session<'_>, direct: Path) -> (u32, u32, u32) {
    let entry = ptr::addr_of!(facts_hart1_entry) as u32;
    if direct == Path::Direct {
        set_boot_address(s, entry | 0x8000_0000);
    }
    let c = &mut s.core_control;
    let index = SECONDARY.hart().index();
    c.pmu_cpu_stall.modify_cpu_stall_sw(|w| {
        w.set_hpcore_sw_stall_code(index, 0xff);
    });
    let mut polls = 0u32;
    while c
        .hp_sys_core_stall_status
        .read_hp_cpu_corestalled_st()
        .hp_reg_core_corestalled_st(index)
        && polls < STALL_POLLS
    {
        polls = polls.saturating_add(1);
    }
    c.hp_sys_clkrst_core.modify_hpcore_ctrl0(index, |w| {
        w.set_reg_core_cpu_clk_en(true);
    });
    c.hp_sys_clkrst_core.modify_hpcore_ctrl0(index, |w| {
        w.set_reg_core_clic_clk_en(true);
    });
    c.hp_sys_clkrst_core.modify_hpcore_ctrl0(index, |w| {
        w.set_reg_core_global_rst_en(false);
    });
    oer_portable_riscv::fence();
    let start = oer_portable_riscv::cycle32();
    if direct == Path::Normal {
        set_boot_address(s, entry);
    }
    (entry, polls, start)
}

/// Wait for hart 1's acknowledgement of `phase`, within `PHASE_CYCLES`.
fn acknowledged(phase: u32) -> bool {
    let start = oer_portable_riscv::cycle32();
    while oer_portable_riscv::cycle32().wrapping_sub(start) < PHASE_CYCLES {
        if ACK.load(Ordering::Acquire) == phase {
            return true;
        }
    }
    false
}

/// 323 (tasks/253, 254): a hart's store of a pending byte stays its own.
/// Line `CROSS_LINE` disabled on both harts and MIE cleared on both. A
/// hundred trials each way: hart 0 stores its own byte pended (and reads
/// it set), hart 1 then reads its own clear, and hart 0 clears its own
/// (and reads it clear); hart 1 stores its own pended (and reads it set),
/// hart 0 then reads its own clear, and hart 1 clears its own (and reads it
/// clear). Records `(0x3a00, trials, hart 0 own set, hart 1 foreign
/// clear)`, `(0x3a01, hart 0 own cleared, hart 1 own set, hart 0 foreign
/// clear)`, `(0x3a02, hart 1 own cleared, enables (hart 0 | hart 1 << 8),
/// hart 1's stage)`, then 232's release words `(0x3a03, routed, first,
/// polls)`.
pub(super) fn stores_stay_their_harts(s: &mut Session<'_>) {
    feed(s);
    park(s);
    oer_portable_riscv::disable_interrupts();
    let (routed, first) = park_routes(s);
    let (Ok(i), Ok(hart)) = (
        Index::new(CROSS_LINE),
        oer_portable_riscv::OnHart::<0>::check(),
    ) else {
        emit(0x3aff, 0, 0, 0);
        return;
    };
    let before = line_state(&s.interrupt_matrix.clic[0], i);
    edge_disabled(&mut s.interrupt_matrix.clic[0], i);
    let state0 = before | line_state(&s.interrupt_matrix.clic[0], i) << 8;
    lend(s, CROSS);
    let (_entry, polls, released) = release(s, Path::Normal);
    // Hart 1's entry runs the ROM's path first: wait for its ready word
    // within the release's own bound.
    let mut ready = 0;
    while ready != CROSS_READY
        && ready != CROSS_NOT_HART1
        && oer_portable_riscv::cycle32().wrapping_sub(released) < WAIT_CYCLES
    {
        ready = FACTS_HART1
            .first()
            .map_or(0, |word| word.load(Ordering::Acquire));
    }
    let own = |s: &Session<'_>| s.interrupt_matrix.clic[0].read_ip(i).ip();
    let mut trials = 0u32;
    let mut own_set = 0u32;
    let mut own_cleared = 0u32;
    let mut foreign_clear = 0u32;
    for k in 0..if ready == CROSS_READY {
        CROSS_TRIALS
    } else {
        0
    } {
        let base = 3 * k;
        s.interrupt_matrix.clic_pending.0.pend_line(i, &hart);
        own_set += u32::from(own(s));
        PHASE.store(base + 1, Ordering::Release);
        if !acknowledged(base + 1) {
            break;
        }
        s.interrupt_matrix.clic_pending.0.clear_line(i, &hart);
        own_cleared += u32::from(!own(s));
        PHASE.store(base + 2, Ordering::Release);
        if !acknowledged(base + 2) {
            break;
        }
        foreign_clear += u32::from(!own(s));
        PHASE.store(base + 3, Ordering::Release);
        if !acknowledged(base + 3) {
            break;
        }
        trials += 1;
    }
    let start = oer_portable_riscv::cycle32();
    let mut stage = 0;
    while stage != CROSS_DONE
        && stage != CROSS_NOT_HART1
        && oer_portable_riscv::cycle32().wrapping_sub(start) < PHASE_CYCLES
    {
        stage = FACTS_HART1
            .first()
            .map_or(0, |word| word.load(Ordering::Acquire));
    }
    park(s);
    revoke();
    emit(0x3a00, trials, own_set, load(CROSS_FOREIGN_CLEAR));
    emit(0x3a01, own_cleared, load(CROSS_OWN_SET), foreign_clear);
    emit(
        0x3a02,
        load(CROSS_OWN_CLEARED),
        state0 | load(CROSS_LINE_STATE) << 16,
        stage,
    );
    emit(0x3a03, routed, first, polls);
    feed(s);
}
