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

const CLIC_DONE: u32 = 0xfac7_1002;
const TRAPPED: u32 = 0xfac7_100f;
const INFO: usize = 70;
const CONFIG: usize = 71;
const THRESHOLD: usize = 72;
const LINES: usize = 73;
const TRAP: usize = 124;
const _: () = if LINES + pac::clic::ctrl::COUNT > TRAP {
    panic!("CLIC lines overlap the trap words");
};
/// Release wait and stall-status polls, both far above the expected path.
const WAIT_CYCLES: u32 = 40_000_000;
const STALL_POLLS: u32 = 100_000;

// SAFETY: the entry/trap stubs only store to FACTS_HART1 and read CSRs; the
// entry installs gp and the reserved hart-1 stack before its only call.
core::arch::global_asm!(
    include_str!("../hart1-entry.s"),
    mstatus = const oer_riscv::machine::MSTATUS,
    mie = const oer_riscv::machine::status::MIE,
    mtvec = const oer_riscv::machine::MTVEC,
    mscratch = const oer_riscv::machine::MSCRATCH,
    mepc = const oer_riscv::machine::MEPC,
    mcause = const oer_riscv::machine::MCAUSE,
    mtvt = const oer_riscv::clic::MTVT,
    mintthresh = const oer_riscv::clic::MINTTHRESH,
    mintstatus = const oer_riscv::clic::MINTSTATUS,
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
    if !clic.is_null() && !observer.is_null() {
        // SAFETY: hart 0 lent these owners for this window and only polls
        // FACTS_HART1 until hart 1 is parked; every access below is a read.
        let (clic, observer) = unsafe { (&*clic, &*observer) };
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
        for n in 0..pac::clic::ctrl::COUNT {
            let Ok(i) = Index::new(n) else { continue };
            let w = clic.read_ctrl(i);
            store(
                LINES + n,
                u32::from(w.ctl())
                    | (w.attr_mode().get() << 8)
                    | (u32::from(w.ip()) << 16)
                    | (u32::from(w.ie()) << 17)
                    | (u32::from(w.attr_shv()) << 18)
                    | (w.attr_trig().get() << 19),
            );
        }
    }
    if let Some(word) = FACTS_HART1.first() {
        word.store(CLIC_DONE, Ordering::Release);
    }
    loop {
        oer_riscv::wait_for_interrupt();
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
    let wdt = &mut s.ble.watchdog;
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
        s.wake_stub.read_lp_store8().lp_scratch8(),
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
    let bank = &mut s.interrupt_matrix.interrupt_core1;
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
    oer_riscv::fence();
    (routed, first)
}

fn set_boot_address(s: &mut Session<'_>, value: u32) {
    s.core_control
        .lp_sys_core_control
        .modify_boot_addr_hp_core(SECONDARY.index(), |w| {
            w.set_boot_addr_hp_core(value);
        });
    oer_riscv::fence();
}

/// Park hart 1 as the session found it: boot word zero, stall requested,
/// clocks gated and global reset held.
fn park(s: &mut Session<'_>) {
    set_boot_address(s, 0);
    super::contain(s);
    oer_riscv::fence();
}

pub(super) fn main_path(s: &mut Session<'_>) {
    run(s, false);
}

pub(super) fn direct_path(s: &mut Session<'_>) {
    run(s, true);
}

fn run(s: &mut Session<'_>, direct: bool) {
    let tag = if direct { 0x2330 } else { 0x2320 };
    feed(s);
    park(s);
    boot_inputs(s, tag);
    let (routed, first) = park_routes(s);
    for word in &FACTS_HART1 {
        word.store(0, Ordering::Relaxed);
    }
    CLIC.store(
        ptr::from_ref(&s.interrupt_matrix.clic).cast_mut(),
        Ordering::Relaxed,
    );
    OBSERVER.store(
        ptr::from_ref(&s.interrupt_matrix.clic_observer).cast_mut(),
        Ordering::Relaxed,
    );
    oer_riscv::fence();
    let entry = ptr::addr_of!(facts_hart1_entry) as u32;
    if direct {
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
    oer_riscv::fence();
    let start = oer_riscv::cycle32();
    if !direct {
        set_boot_address(s, entry);
    }
    let mut stage = 0;
    let mut elapsed = 0;
    while elapsed < WAIT_CYCLES {
        stage = FACTS_HART1
            .first()
            .map_or(0, |word| word.load(Ordering::Acquire));
        let trapped = FACTS_HART1
            .get(TRAP)
            .is_some_and(|word| word.load(Ordering::Acquire) == TRAPPED);
        elapsed = oer_riscv::cycle32().wrapping_sub(start);
        if stage == CLIC_DONE || trapped {
            break;
        }
    }
    park(s);
    CLIC.store(ptr::null_mut(), Ordering::Relaxed);
    OBSERVER.store(ptr::null_mut(), Ordering::Relaxed);
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
