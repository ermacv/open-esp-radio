//! PAC-only experiments. Add a leaf to PROBES; production has no probe hooks.
#![allow(
    unsafe_code,
    reason = "standalone probe entry, trap ABI and retained SRAM"
)]
const SECONDARY: pac::SecondaryHart = const { pac::Hart::SECONDARIES[0] };

use core::{
    ptr,
    sync::atomic::{AtomicPtr, AtomicU32, Ordering},
};
use oer_esp32s31_pac as pac;
use oer_mmio::{Bits, Index};

mod ble;
mod ble_wave2;
mod generic;
mod radio_local;

include!(concat!(env!("OUT_DIR"), "/geometry.rs"));
const CPU_LINE: Index<{ pac::clic::ctrl::COUNT }> = Index::constant::<LINE>();
const SOFTWARE_SOURCE: Index<{ pac::interrupt_core::route::COUNT }> =
    Index::constant::<{ pac::Interrupt::CPU_INTR_FROM_CPU_2 as usize }>();

// SAFETY: the image owns entry/traps and the generated vectors; the included
// production assembly uses its unchanged ABI and the linker-owned stacks.
core::arch::global_asm!(
    include_str!(concat!(env!("OUT_DIR"), "/vectors.s")),
    include_str!("../../../../chips/espressif/staged-runtime/src/vectors.s"),
    fs = const oer_riscv::machine::status::FS_MASK,
    mie = const oer_riscv::machine::status::MIE,
    mintthresh = const oer_riscv::clic::MINTTHRESH,
    tcontrol = const oer_riscv::trigger::TCONTROL,
    mte = const oer_riscv::trigger::TCONTROL_MTE,
    mpte_shift = const (oer_riscv::trigger::TCONTROL_MPTE.trailing_zeros() - oer_riscv::trigger::TCONTROL_MTE.trailing_zeros()),
);
// SAFETY: the loader enters this symbol on hart 0 only. It establishes gp/sp,
// zeroes the image's BSS (not retained RAM) before any Rust access, and never returns.
core::arch::global_asm!(
    r#"
.section .text._start,"ax"
.global _start
_start:
csrci mstatus, {mie}
.option push
.option norelax
la gp, __global_pointer$
.option pop
la sp, __runtime_cpu0_task_stack_top
la t0, __bss_start
la t1, __bss_end
1: bgeu t0, t1, 2f
sw zero, 0(t0)
addi t0, t0, 4
j 1b
2: j facts_main
"#,
    mie = const oer_riscv::machine::status::MIE,
);

// SAFETY: these controlled admission entries preserve C-ABI callee registers
// and write only the caller's six-word output. Their live shared PAC borrow
// reaches the typed Rust raise thunk; the trap ABI preserves interrupted state.
core::arch::global_asm!(
    include_str!("threshold-entry.s"),
    mie = const oer_riscv::machine::status::MIE,
    mintthresh = const oer_riscv::clic::MINTTHRESH,
);

// SAFETY: addresses only; definitions and extents are supplied by facts.x.
unsafe extern "C" {
    safe static _start_trap: [u8; 0];
    safe static _runtime_mtvt: [u8; 0];
    safe static __runtime_cpu0_irq_stack_top: [u8; 0];
}

type ThresholdEntry = unsafe extern "C" fn(*mut [u32; 6], *const pac::hp_system::FromCpu2, u32);
// SAFETY: the assembly defines 25 rows of eight valid C-ABI entry pointers.
// Each requires masked interrupts, an installed trap stack, a live shared PAC
// source and an exclusive six-word output; threshold_raising supplies these.
unsafe extern "C" {
    safe static _facts_threshold_entries: [[ThresholdEntry; 8]; 25];
}

/// Word 0: completion marker; word 1: number of four-word records starting at 2.
// SAFETY: unique exported result buffer; only this image writes it.
#[unsafe(no_mangle)]
static FACTS_OUTPUT: [AtomicU32; 514] = [const { AtomicU32::new(0) }; 514];
/// The debugger publishes a command while the hart is halted, then resumes
/// the same context. u32::MAX is idle; _start is entered only by the loader.
// SAFETY: unique exported selector; debugger writes only while hart 0 is halted.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".data.facts_request")]
static FACTS_REQUEST: AtomicU32 = AtomicU32::new(u32::MAX);
/// Selection 9 sweeps one K per command to keep the existing output bounded.
// SAFETY: unique selector; the debugger writes only while hart 0 is halted.
#[unsafe(no_mangle)]
static FACTS_THRESHOLD_K: AtomicU32 = AtomicU32::new(0);

// SAFETY: these aligned LP words are excluded from loading/startup clearing;
// all u32 bit patterns are valid, and only the masked thread publishes them.
#[unsafe(link_section = ".noinit.facts")]
static RETAINED: [AtomicU32; 12] = [const { AtomicU32::new(0) }; 12];
static IRQ_COUNT: AtomicU32 = AtomicU32::new(0);
static IRQ_CYCLE: AtomicU32 = AtomicU32::new(0);
static IRQ_PC: AtomicU32 = AtomicU32::new(0);
static SOURCE: AtomicPtr<pac::hp_system::FromCpu2> = AtomicPtr::new(ptr::null_mut());

fn emit(tag: u32, a: u32, b: u32, c: u32) {
    let record = FACTS_OUTPUT[1].load(Ordering::Relaxed);
    let at = 2usize.saturating_add((record as usize).saturating_mul(4));
    if let Some(words) = FACTS_OUTPUT.get(at..at.saturating_add(4)) {
        for (word, value) in words.iter().zip([tag, a, b, c]) {
            word.store(value, Ordering::Relaxed);
        }
        FACTS_OUTPUT[1].store(record.saturating_add(1), Ordering::Release);
    } else {
        FACTS_OUTPUT[0].store(0xbad00005, Ordering::Release);
        halt();
    }
}

fn halt() -> ! {
    oer_riscv::disable_interrupts();
    loop {
        oer_riscv::wait_for_interrupt();
    }
}

// SAFETY: generated vector aliases call the handler using the C ABI;
// SOURCE points to the live shared PAC source until all interrupts are disabled.
#[unsafe(no_mangle)]
extern "C" fn facts_irq() {
    let cycle = oer_riscv::cycle32();
    IRQ_CYCLE.store(cycle, Ordering::Relaxed);
    IRQ_PC.store(oer_riscv::read_mepc() as u32, Ordering::Relaxed);
    if generic::interrupt() {
        IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let source = SOURCE.load(Ordering::Acquire);
    if !source.is_null() {
        // SAFETY: published from main's live source_owner; this
        // shared-register owner is only accessed through &self until IRQs stop.
        unsafe { &*source }.store_hp_cpu_int_from_cpu_2(false);
    }
    let count = IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
    if count >= 8 {
        emit(0xee, oer_riscv::read_mcause() as u32, count, cycle);
        FACTS_OUTPUT[0].store(0xbad00001, Ordering::Release);
        halt();
    }
}

// SAFETY: the included trap entry calls this ABI with a pointer to its frame;
// the probe only records architectural diagnostics and never returns.
#[unsafe(no_mangle)]
extern "C" fn oer_exception_handler(_frame: usize) -> ! {
    emit(
        0xef,
        oer_riscv::read_mcause() as u32,
        oer_riscv::read_mepc() as u32,
        oer_riscv::read_mtval() as u32,
    );
    FACTS_OUTPUT[0].store(0xbad00002, Ordering::Release);
    halt()
}

struct Session<'a> {
    source: &'a pac::hp_system::FromCpu2,
    interrupt_matrix: pac::ownership::InterruptMatrixPeripherals,
    core_control: pac::ownership::CoreControlPeripherals,
    uart0_clock: pac::ownership::Uart0ClockPeripherals,
    reset_cause: pac::ownership::ResetCausePeripherals,
    ble: ble::Ble,
    radio: radio_local::Radio,
    generic: generic::Hardware,
}
struct Probe {
    id: u32,
    run: fn(&mut Session<'_>),
}
// The sole registration point. Reset is selected explicitly; it cannot run
// accidentally as part of the non-reset batch (request 0).
const PROBES: &[Probe] = &[
    Probe {
        id: 96,
        run: ble_wave2::lp_clear,
    },
    Probe {
        id: 97,
        run: ble_wave2::sleep16,
    },
    Probe {
        id: 98,
        run: ble_wave2::sleep_other,
    },
    Probe {
        id: 99,
        run: ble_wave2::shutdown_soft,
    },
    Probe {
        id: 100,
        run: ble_wave2::shutdown_wrap,
    },
    Probe {
        id: 91,
        run: ble_wave2::mapping,
    },
    Probe {
        id: 92,
        run: ble_wave2::masks,
    },
    Probe {
        id: 93,
        run: ble_wave2::controls,
    },
    Probe {
        id: 94,
        run: ble_wave2::groups,
    },
    Probe {
        id: 95,
        run: ble_wave2::arrivals,
    },
    Probe {
        id: 90,
        run: ble_wave2::prerequisite,
    },
    Probe {
        id: 82,
        run: radio_local::ieee_timers,
    },
    Probe {
        id: 83,
        run: radio_local::wifi_timers,
    },
    Probe {
        id: 80,
        run: ble::compare_ack,
    },
    Probe {
        id: 81,
        run: ble::two_wraps,
    },
    Probe {
        id: 40,
        run: generic::matrix,
    },
    Probe {
        id: 41,
        run: generic::usb,
    },
    Probe {
        id: 42,
        run: generic::systimer0,
    },
    Probe {
        id: 43,
        run: generic::systimer1,
    },
    Probe {
        id: 44,
        run: generic::timer0,
    },
    Probe {
        id: 32,
        run: ble::timer_reset_on,
    },
    Probe {
        id: 33,
        run: ble::timer_reset_off,
    },
    Probe {
        id: 34,
        run: ble::controller_reset_on,
    },
    Probe {
        id: 35,
        run: ble::controller_reset_off,
    },
    Probe {
        id: 20,
        run: ble::inherited,
    },
    Probe {
        id: 31,
        run: ble::btlc_prefix,
    },
    Probe {
        id: 30,
        run: ble::rollover_gates,
    },
    Probe {
        id: 29,
        run: ble::software_pending,
    },
    Probe {
        id: 28,
        run: ble::counter_controls,
    },
    Probe {
        id: 27,
        run: ble::bit_one,
    },
    Probe {
        id: 21,
        run: ble::nrt_controls,
    },
    Probe {
        id: 22,
        run: ble::nrt_arrivals,
    },
    Probe {
        id: 23,
        run: ble::lp_causes,
    },
    Probe {
        id: 24,
        run: ble::lp_stop,
    },
    Probe {
        id: 25,
        run: ble::lp_wrap,
    },
    Probe {
        id: 26,
        run: ble::lp_modes,
    },
    Probe {
        id: 2,
        run: geometry,
    },
    Probe { id: 3, run: config },
    Probe {
        id: 6,
        run: boot_address,
    },
    Probe { id: 1, run: edge },
    Probe {
        id: 4,
        run: threshold,
    },
    Probe {
        id: 5,
        run: latency,
    },
    Probe { id: 7, run: reset },
    Probe {
        id: 8,
        run: after_reset,
    },
    Probe {
        id: 9,
        run: threshold_raising,
    },
];

// SAFETY: _start initializes the image and calls this once on hart 0.
#[unsafe(no_mangle)]
extern "C" fn facts_main() -> ! {
    oer_riscv::disable_interrupts();
    let Some(p) = pac::take() else { halt() };
    let source_owner = p.software_interrupt_2.from_cpu2;
    let mut s = Session {
        source: &source_owner,
        interrupt_matrix: p.interrupt_matrix,
        core_control: p.core_control,
        uart0_clock: p.uart0_clock,
        reset_cause: p.reset_cause,
        radio: radio_local::Radio {
            tick: p.radio_phy.modem_lpcon_phy_tick,
            local_time: p.mac_local_time.wifi_mac_local_time,
            status: p.wifi_mac.wifi_mac_tsf_status,
            zb: p.ieee802154,
            power: p.wifi_interrupts.wifi_mac_power_interrupt,
            timer: p.wifi_mac.wifi_mac_tsf_timer,
            load: p.wifi_mac.wifi_mac_tsf_load,
            rtc: p.wifi_mac.wifi_mac_rtc_timer_update,
            aux: p.wifi_mac.wifi_mac_aux_tsf_control,
        },
        generic: generic::Hardware {
            usb: p.usb_serial_jtag.usb_serial_jtag_task,
            usb_irq: p.usb_serial_jtag.usb_serial_jtag_irq,
            systimer: p.system_timer,
            systimer_clock: p.soc.hp_sys_clkrst_systimer,
            timer: p.timer_group0,
            watchdog: p.watchdog.timg0_wdt,
            watchdog_clock: p.watchdog.hp_sys_clkrst_timg0,
        },
        ble: ble::Ble {
            bank: p.bluetooth_interrupts.bluetooth_interrupt_bank,
            core: p.bluetooth.bluetooth_controller_core,
            timer: p.bluetooth_modem_lp_timer.btdm_runtime_control,
            timer_status: p.bluetooth_modem_lp_timer.btdm_lp_timer_status,
            clocks: p.radio_phy.modem_syscon_radio,
            lp_clock: p.radio_phy.modem_lpcon_shared_clock,
            gates: p.chip_clocks.modem_lpcon_chip_clocks,
            upstream: p.radio_phy.hp_sys_clkrst_radio,
            pmu: p.radio_phy.pmu_radio,
            watchdog: p.watchdog.lp_wdt,
            positional: p.bluetooth.ble_hw_positional_word1ff0,
            etm: p.modem_etm.modem_etm_bluetooth,
            task_irq: p.bluetooth.btmac_ble_phy_init,
        },
    };
    let source = ptr::addr_of!(source_owner).cast_mut();
    SOURCE.store(source, Ordering::Release);
    // One take for the entire loader session. Commands reuse these owners;
    // the debugger never re-enters _start merely to select another leaf.
    let prior = retained_record();
    let mut request = if prior[0] == 0xfac70007 && prior[1] == 0x687731 {
        8
    } else {
        u32::MAX
    };
    loop {
        FACTS_OUTPUT[1].store(0, Ordering::Relaxed);
        if request == u32::MAX {
            FACTS_OUTPUT[0].store(0xfac70000, Ordering::Release);
        } else {
            // A slow/incomplete leaf must not retain the previous command's
            // completion marker while a bounded debugger capture observes it.
            FACTS_OUTPUT[0].store(0xfac70002, Ordering::Release);
            let mut ran = false;
            for probe in PROBES {
                if request == probe.id || (request == 0 && probe.id < 7) {
                    ran = true;
                    (probe.run)(&mut s);
                }
            }
            if !ran {
                emit(0xed, request, 0, 0);
                FACTS_OUTPUT[0].store(0xbad00004, Ordering::Release);
                halt();
            }
            oer_riscv::disable_interrupts();
            FACTS_OUTPUT[0].store(0xfac70001, Ordering::Release);
        }
        FACTS_REQUEST.store(u32::MAX, Ordering::Release);
        loop {
            // WFI is idle, not a measurement loop. A debugger resumes the
            // halted context after publishing the next command in SRAM.
            oer_riscv::wait_for_interrupt();
            request = FACTS_REQUEST.load(Ordering::Acquire);
            if request != u32::MAX {
                break;
            }
        }
    }
}

fn geometry(s: &mut Session<'_>) {
    let clic = &s.interrupt_matrix.clic;
    let info = clic.read_int_info();
    emit(
        0x20,
        info.num_int().get(),
        info.ctlbits().get(),
        u32::from(info.version()),
    );
    for n in 0..pac::clic::ctrl::COUNT {
        let Ok(i) = Index::new(n) else { continue };
        let w = clic.read_ctrl(i);
        emit(
            0x200 + n as u32,
            w.ctl().into(),
            w.attr_mode().get(),
            u32::from(w.ip())
                | (u32::from(w.ie()) << 1)
                | (u32::from(w.attr_shv()) << 2)
                | (w.attr_trig().get() << 3),
        );
    }
}
fn config(s: &mut Session<'_>) {
    let w = s.interrupt_matrix.clic_observer.read_int_config();
    emit(
        0x30,
        w.mnlbits().get(),
        u32::from(w.nvbits()),
        w.nmbits().get(),
    );
}

fn contain(s: &mut Session<'_>) {
    let c = &mut s.core_control;
    // SOURCE: HAL cores::park_hart1 and boot::soc::contain_hart1; no release.
    c.pmu_cpu_stall.modify_cpu_stall_sw(|w| {
        w.set_hpcore_sw_stall_code(SECONDARY.hart().index(), 0x86);
    });
    c.hp_sys_clkrst_core
        .modify_hpcore_ctrl0(SECONDARY.hart().index(), |w| {
            w.set_reg_core_cpu_clk_en(false)
                .set_reg_core_clic_clk_en(false)
                .set_reg_core_global_rst_en(true);
        });
}
fn boot_address(s: &mut Session<'_>) {
    contain(s);
    let c = &mut s.core_control;
    for value in [0, 0x2f001234, 0x01234567, 0xfedcba98, 0] {
        c.lp_sys_core_control
            .modify_boot_addr_hp_core(SECONDARY.index(), |w| {
                w.set_boot_addr_hp_core(value);
            });
        oer_riscv::fence();
        let read = c
            .lp_sys_core_control
            .read_boot_addr_hp_core(SECONDARY.index())
            .boot_addr_hp_core();
        let held = c
            .hp_sys_clkrst_core
            .read_hpcore_ctrl0(SECONDARY.hart().index());
        emit(
            0x60,
            value,
            read,
            u32::from(held.reg_core_global_rst_en()) + 2 * u32::from(held.reg_core_cpu_clk_en()),
        );
    }
    // Same reset preparation order as production, with an explicit readback
    // inserted before the pulse. Hart1 remains globally reset/clock gated.
    emit(
        0x61,
        c.lp_sys_core_control
            .read_boot_addr_hp_core(SECONDARY.index())
            .boot_addr_hp_core(),
        0,
        0,
    );
    c.lp_aon_clkrst_core_reset
        .trigger_hpcore_reset_ctrl_hpcore_sw_reset(SECONDARY.hart().index());
    emit(
        0x62,
        c.lp_sys_core_control
            .read_boot_addr_hp_core(SECONDARY.index())
            .boot_addr_hp_core(),
        0,
        0,
    );
}

fn setup(s: &mut Session<'_>) {
    oer_riscv::disable_interrupts();
    contain(s);
    let matrix = &mut s.interrupt_matrix;
    for n in 0..pac::interrupt_core::route::COUNT {
        if let Ok(i) = Index::new(n) {
            matrix.interrupt_core0.modify_route(i, |w| {
                w.set_map(Bits::constant::<
                    { pac::interrupt_core::route::Map::Disconnected.bits() },
                >());
            });
        }
    }
    for n in 0..pac::clic::ctrl::COUNT {
        if let Ok(i) = Index::new(n) {
            matrix.clic.modify_ctrl(i, |w| {
                w.set_ie(false);
            });
        }
    }
    matrix.clic_observer.modify_int_config(|w| {
        w.set_mnlbits(Bits::constant::<LEVEL_BITS>());
    });
    // SAFETY: unchanged runtime trap ABI; generated table and both exclusive
    // stacks remain in SRAM for this whole image, with IRQ stack below task stack.
    unsafe {
        oer_riscv::install_trap_stack(
            ptr::addr_of!(_start_trap) as usize | oer_riscv::machine::MTVEC_CLIC_MODE,
            ptr::addr_of!(_runtime_mtvt) as usize,
            ptr::addr_of!(__runtime_cpu0_irq_stack_top) as usize,
        );
    }
    oer_riscv::restore_interrupt_threshold(0);
    IRQ_COUNT.store(0, Ordering::Relaxed);
}
fn line(s: &mut Session<'_>, control: u8, edge: bool) {
    let i = CPU_LINE;
    s.interrupt_matrix.clic.modify_ctrl(i, |w| {
        w.set_ie(false);
    });
    s.interrupt_matrix.clic.modify_ctrl(i, |w| {
        w.set_attr_shv(true)
            .set_attr_trig(if edge {
                pac::clic::ctrl::AttrTrig::PositiveEdge
            } else {
                pac::clic::ctrl::AttrTrig::PositiveLevel
            })
            .set_ctl(control);
    });
    s.interrupt_matrix.clic.modify_ctrl(i, |w| {
        w.set_ie(true);
    });
}
fn wait_window() {
    // A bounded observation window, not a service-time or stress qualification.
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < 4096 {}
}
fn edge(s: &mut Session<'_>) {
    setup(s);
    line(s, CONTROLS[1], true);
    let i = CPU_LINE;
    s.interrupt_matrix.clic.modify_ctrl(i, |w| {
        w.set_ip(true);
    });
    let pending = s.interrupt_matrix.clic.read_ctrl(i).ip();
    oer_riscv::enable_interrupts();
    wait_window();
    oer_riscv::disable_interrupts();
    emit(
        0x10,
        u32::from(pending),
        IRQ_COUNT.load(Ordering::Relaxed),
        u32::from(s.interrupt_matrix.clic.read_ctrl(i).ip()),
    );
    wait_window();
}
fn route_source(s: &mut Session<'_>) {
    let i = SOFTWARE_SOURCE;
    s.interrupt_matrix.interrupt_core0.modify_route(i, |w| {
        w.set_map(Bits::constant::<{ LINE as u32 }>());
    });
}
fn raise(s: &Session<'_>) {
    s.source.store_hp_cpu_int_from_cpu_2(true);
    let _ = s.source.read_hp_cpu_int_from_cpu_2();
}

fn threshold(s: &mut Session<'_>) {
    for (level, &control) in CONTROLS.iter().enumerate() {
        setup(s);
        route_source(s);
        line(s, control, false);
        oer_riscv::restore_interrupt_threshold(u8::MAX);
        raise(s);
        oer_riscv::enable_interrupts();
        // Pending source and MIE are already live: only this CSR write lowers
        // admission. The public restore has three NOPs and no CSR readback.
        oer_riscv::restore_interrupt_threshold(CONTROLS[1]);
        wait_window();
        oer_riscv::disable_interrupts();
        let masked = IRQ_COUNT.load(Ordering::Relaxed);
        oer_riscv::enable_interrupts();
        oer_riscv::restore_interrupt_threshold(0);
        wait_window();
        oer_riscv::disable_interrupts();
        emit(
            0x40 + level as u32,
            control.into(),
            masked,
            IRQ_COUNT.load(Ordering::Relaxed),
        );
    }
}
fn latency(s: &mut Session<'_>) {
    setup(s);
    route_source(s);
    line(s, CONTROLS[1], false);
    for sample in 0..5 {
        IRQ_COUNT.store(0, Ordering::Relaxed);
        oer_riscv::enable_interrupts();
        let start = oer_riscv::cycle32();
        raise(s);
        wait_window();
        oer_riscv::disable_interrupts();
        emit(
            0x50 + sample,
            IRQ_CYCLE.load(Ordering::Relaxed).wrapping_sub(start),
            IRQ_COUNT.load(Ordering::Relaxed),
            0,
        );
    }
}

// SAFETY: the admission entries call this with main's live shared PAC owner.
// Keep it out of line so its one typed store and return can be audited in ELF.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.facts_threshold_raise")]
#[inline(never)]
extern "C" fn facts_threshold_raise(source: &pac::hp_system::FromCpu2) {
    source.store_hp_cpu_int_from_cpu_2(true);
}

fn threshold_raising(s: &mut Session<'_>) {
    let k = FACTS_THRESHOLD_K.load(Ordering::Acquire);
    let Some(entries) = _facts_threshold_entries.get(k as usize) else {
        emit(0xed, 9, k, 0);
        return;
    };
    setup(s);
    route_source(s);
    line(s, CONTROLS[0], false);
    emit(0x95, k, u32::from(CONTROLS[0]), u32::from(CONTROLS[1]));
    for (variant, &entry) in entries.iter().enumerate() {
        for sample in 0..3 {
            oer_riscv::disable_interrupts();
            s.source.store_hp_cpu_int_from_cpu_2(false);
            oer_riscv::fence();
            oer_riscv::restore_interrupt_threshold(0);
            IRQ_COUNT.store(0, Ordering::Relaxed);
            IRQ_CYCLE.store(0, Ordering::Relaxed);
            IRQ_PC.store(0, Ordering::Relaxed);
            let pending = s.interrupt_matrix.clic.read_ctrl(CPU_LINE).ip();
            let mut marks = [0; 6];
            // SAFETY: setup installed the exclusive IRQ stack; MIE is masked,
            // source is main's live shared PAC owner, marks is exclusive. The
            // entry enables MIE, calls the typed raise without readback, runs
            // unrolled K NOPs, raises the ceiling and masks MIE before returning.
            unsafe {
                entry(
                    &raw mut marks,
                    ptr::from_ref(s.source),
                    u32::from(CONTROLS[1]),
                )
            };
            let count = IRQ_COUNT.load(Ordering::Relaxed);
            let pc = IRQ_PC.load(Ordering::Relaxed);
            let cycle = IRQ_CYCLE.load(Ordering::Relaxed);
            // Let the source propagate with MIE off and the ceiling still high.
            // This pending sample is outside the timed instruction window.
            wait_window();
            let still_pending = s.interrupt_matrix.clic.read_ctrl(CPU_LINE).ip();
            oer_riscv::restore_interrupt_threshold(0);
            oer_riscv::enable_interrupts();
            wait_window();
            oer_riscv::disable_interrupts();
            emit(0x90, variant as u32, sample, u32::from(pending));
            emit(0x91, marks[0], marks[1], cycle);
            emit(0x92, count, pc, marks[2]);
            emit(0x93, marks[3], marks[4], marks[5]);
            emit(
                0x94,
                u32::from(still_pending),
                IRQ_COUNT.load(Ordering::Relaxed),
                u32::from(CONTROLS[0]),
            );
        }
    }
}
fn reset(s: &mut Session<'_>) {
    contain(s);
    let c = &mut s.core_control;
    let lp = c
        .lp_sys_core_control
        .read_sys_ctrl()
        .lp_core_etm_wakeup_flag();
    let uart = &mut s.uart0_clock.hp_sys_clkrst_uart0;
    // SOURCE: production restore_uart0_clock; intentionally perturb the divider
    // while keeping UART0 live, then restore the same typed reset preparation.
    uart.modify_uart0_ctrl0(|w| {
        w.set_reg_uart0_sclk_div_num(3);
    });
    let perturbed = uart.read_uart0_ctrl0().reg_uart0_sclk_div_num();
    uart.modify_uart0_ctrl0(|w| {
        w.set_reg_uart0_clk_src_sel(Bits::constant::<0>())
            .set_reg_uart0_sclk_div_num(0)
            .set_reg_uart0_sclk_div_numerator(0)
            .set_reg_uart0_sclk_div_denominator(0)
            .set_reg_uart0_clk_en(true);
    });
    let u = uart.read_uart0_ctrl0();
    emit(
        0x70,
        u.reg_uart0_clk_src_sel().get(),
        u32::from(u.reg_uart0_sclk_div_num()),
        u32::from(u.reg_uart0_clk_en()),
    );
    c.lp_sys_core_control
        .modify_boot_addr_hp_core(SECONDARY.index(), |w| {
            w.set_boot_addr_hp_core(0);
        });
    oer_riscv::fence();
    c.lp_aon_clkrst_core_reset
        .trigger_hpcore_reset_ctrl_hpcore_sw_reset(SECONDARY.hart().index());
    c.pmu_cpu_stall.modify_cpu_stall_sw(|w| {
        w.set_hpcore_sw_stall_code(SECONDARY.hart().index(), 0x86);
    });
    oer_riscv::fence();
    FACTS_OUTPUT[0].store(0xfac70007, Ordering::Release);
    let held = c
        .hp_sys_clkrst_core
        .read_hpcore_ctrl0(SECONDARY.hart().index());
    for (word, value) in RETAINED.iter().zip([
        0xfac70007,
        0x687731,
        u32::from(lp),
        u.reg_uart0_clk_src_sel().get(),
        u32::from(u.reg_uart0_sclk_div_num()),
        u32::from(u.reg_uart0_clk_en()),
        c.lp_sys_core_control
            .read_boot_addr_hp_core(SECONDARY.index())
            .boot_addr_hp_core(),
        u32::from(held.reg_core_global_rst_en()) + 2 * u32::from(held.reg_core_cpu_clk_en()),
        u32::from(perturbed),
        u32::from(u.reg_uart0_sclk_div_numerator()),
        u32::from(u.reg_uart0_sclk_div_denominator()),
        1,
    ]) {
        word.store(value, Ordering::Relaxed);
    }
    oer_riscv::fence();
    c.lp_sys_core_control.trigger_sys_ctrl_sys_sw_rst();
    halt()
}
fn retained_record() -> [u32; 12] {
    let mut record = [0; 12];
    for (value, word) in record.iter_mut().zip(&RETAINED) {
        *value = word.load(Ordering::Relaxed);
    }
    record
}
fn after_reset(s: &mut Session<'_>) {
    let record = retained_record();
    emit(0x80, record[0], record[1], record[2]);
    emit(0x83, record[3], record[4], record[5]);
    emit(0x84, record[6], record[7], record[11]);
    emit(0x85, record[8], record[9], record[10]);
    config(s);
    let r = &s.reset_cause.lp_aon_reset_cause;
    emit(
        0x81,
        r.read_hpcore_reset_cause(pac::BOOT_HART.index())
            .hpcore_reset_cause()
            .get(),
        r.read_hpcore_reset_cause(SECONDARY.hart().index())
            .hpcore_reset_cause()
            .get(),
        u32::from(
            s.core_control
                .lp_sys_core_control
                .read_sys_ctrl()
                .lp_core_etm_wakeup_flag(),
        ),
    );
    let u = s.uart0_clock.hp_sys_clkrst_uart0.read_uart0_ctrl0();
    emit(
        0x82,
        u.reg_uart0_clk_src_sel().get(),
        u32::from(u.reg_uart0_sclk_div_num()),
        u32::from(u.reg_uart0_clk_en()),
    );
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    FACTS_OUTPUT[0].store(0xbad00003, Ordering::Release);
    halt()
}
