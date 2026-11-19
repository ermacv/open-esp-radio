//! Selections 310-316 (tasks/187): CLIC levels 1 to 7 nesting on hart 0,
//! the software entry of a level through an edge line's pending bit, and
//! the cycles from a higher handler's pend to the lower line's entry.
//!
//! Lines are external lines `LINE..`, edge-triggered and hardware-vectored,
//! with no routed source unless a case names one (FROM_CPU2). A pend is the
//! typed word read-modify-write of the line's own `CTRL` word setting `IP`,
//! with MIE cleared across the access: the CLIC word belongs to this hart
//! alone and no handler runs between its read and its write. Every handler
//! reaches the CLIC through the thread's loan, one access at a time under the
//! same mask, so no two borrows of it are ever live together.
//!
//! The handler records events (enter, exit, mark) with the line from mcause
//! and the level from mintstatus; the host decodes their order. Nothing here
//! decides a verdict.
use super::{LEVEL_BITS, LINE, SOURCE, Session, emit, generic::feed, setup};
use core::{
    ptr,
    sync::atomic::{AtomicPtr, AtomicU32, Ordering},
};
use oer_chips_esp32s31_pac as pac;
use oer_portable_mmio::{Bits, Index};

/// The control byte of level `n` (1 to 8) with `LEVEL_BITS` level bits:
/// the level code in the top bits, the unimplemented low bits ones.
const LEVEL_CONTROL: [u8; 8] = [
    control(1),
    control(2),
    control(3),
    control(4),
    control(5),
    control(6),
    control(7),
    control(8),
];
const fn control(level: u32) -> u8 {
    ((level << (8 - LEVEL_BITS)) - 1) as u8
}
const _: () = const {
    if LEVEL_BITS != 3 || LINE + 32 > pac::clic::ip::COUNT {
        panic!("levels probe: three level bits and 32 lines from LINE");
    }
};

/// Q1: the primary line of level n is `PRIMARY + n - 1`, its sibling at
/// the same level `SIBLING + n - 1`.
const PRIMARY: u32 = LINE as u32;
const SIBLING: u32 = PRIMARY + 7;
/// Q2 and Q3 lines.
const LOW: u32 = PRIMARY + 14;
const MIDDLE: u32 = PRIMARY + 15;
const HIGH: u32 = PRIMARY + 16;
/// Q2.4: the pended line between a pending edge line and a hardware source.
const NEIGHBOUR_PENDING: u32 = PRIMARY + 17;
const TARGET: u32 = PRIMARY + 18;
const NEIGHBOUR_SOURCE: u32 = PRIMARY + 19;

const REPEATS: u32 = 10;
const SAMPLES: u32 = 100;
/// A bounded observation window after MIE is set, in cycles.
const WINDOW: u32 = 60_000;
/// Entries before a storm guard disables the entered line.
const ENTRY_LIMIT: u32 = 64;
const EVENT_CAPACITY: usize = 64;

const ENTER: u32 = 0;
const EXIT: u32 = 1;
const MARK: u32 = 2;
const STRAY: u32 = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum Mode {
    Idle = 0,
    Nesting = 1,
    FromHigher = 2,
    FromLower = 3,
    OwnHandler = 4,
    Neighbours = 5,
    TwiceFromHigher = 6,
    Latency = 7,
}

static ACTIVE: AtomicU32 = AtomicU32::new(Mode::Idle as u32);
static CLIC: AtomicPtr<pac::clic::Clic> = AtomicPtr::new(ptr::null_mut());
static EVENTS: [AtomicU32; EVENT_CAPACITY] = [const { AtomicU32::new(0) }; EVENT_CAPACITY];
static EVENT_COUNT: AtomicU32 = AtomicU32::new(0);
static ENTRIES: AtomicU32 = AtomicU32::new(0);
/// Per-line entry counts of Q2.4's three lines and Q2.3's own line.
static LINE_ENTRIES: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
static PEND_CYCLE: AtomicU32 = AtomicU32::new(0);
static ENTRY_CYCLE: AtomicU32 = AtomicU32::new(0);

fn mode() -> Mode {
    match ACTIVE.load(Ordering::Acquire) {
        1 => Mode::Nesting,
        2 => Mode::FromHigher,
        3 => Mode::FromLower,
        4 => Mode::OwnHandler,
        5 => Mode::Neighbours,
        6 => Mode::TwiceFromHigher,
        7 => Mode::Latency,
        _ => Mode::Idle,
    }
}

fn index(line: u32) -> Option<Index<{ pac::clic::ip::COUNT }>> {
    Index::new(line as usize).ok()
}

fn log(kind: u32, line: u32) {
    let level = (oer_portable_riscv::read_mintstatus() as u32)
        >> oer_portable_riscv::clic::MINTSTATUS_MIL_SHIFT;
    let at = EVENT_COUNT.fetch_add(1, Ordering::Relaxed) as usize;
    if let Some(slot) = EVENTS.get(at) {
        slot.store(
            kind << 14 | (line & 0x3f) << 8 | (level & 0xff),
            Ordering::Relaxed,
        );
    }
}

/// One access to the lent CLIC with MIE cleared; MIE is set again after it.
/// Called only while MIE is set (the window, or a handler).
fn masked<R>(f: impl FnOnce(&mut pac::clic::Clic) -> R) -> Option<R> {
    oer_portable_riscv::disable_interrupts();
    let clic = CLIC.load(Ordering::Acquire);
    let out = if clic.is_null() {
        None
    } else {
        // SAFETY: the thread lends its exclusive CLIC owner for the window
        // and does not use its own borrow until it revokes the loan with MIE
        // cleared. Every access here runs with MIE cleared on this one hart,
        // so this is the only live borrow derived from the loan.
        Some(f(unsafe { &mut *clic }))
    };
    oer_portable_riscv::enable_interrupts();
    out
}

/// The pend: the line's own word, read, `IP` set, written.
fn pend_in(clic: &mut pac::clic::Clic, line: u32) {
    if let Some(i) = index(line) {
        clic.modify_ip(i, |w| {
            w.set_ip(true);
        });
    }
}

fn pend(line: u32) {
    let _ = masked(|clic| pend_in(clic, line));
    log(MARK, line);
}

fn source() -> Option<&'static pac::hp_system::FromCpu2> {
    let source = SOURCE.load(Ordering::Acquire);
    // SAFETY: published by main from its live source owner, which outlives
    // every command; it is accessed only through `&self`.
    (!source.is_null()).then(|| unsafe { &*source })
}

fn raise_source() {
    if let Some(s) = source() {
        s.store_hp_cpu_int_from_cpu_2(true);
    }
}

fn count(slot: usize) {
    if let Some(c) = LINE_ENTRIES.get(slot) {
        c.fetch_add(1, Ordering::Relaxed);
    }
}

/// The facts handler's first call: whether an entry belonged to this probe.
pub(super) fn interrupt(cycle: u32) -> bool {
    let mode = mode();
    if mode == Mode::Idle {
        return false;
    }
    // SOURCE: RISC-V CLIC specification 3f8ee744, mcause.exccode[11:0].
    let line = (oer_portable_riscv::read_mcause() as u32) & 0xfff;
    if ENTRIES.fetch_add(1, Ordering::Relaxed) >= ENTRY_LIMIT {
        log(STRAY, line);
        let _ = masked(|clic| {
            if let Some(i) = index(line) {
                clic.modify_ie(i, |w| {
                    w.set_ie(false);
                });
            }
        });
        if let Some(s) = source() {
            s.store_hp_cpu_int_from_cpu_2(false);
        }
        return true;
    }
    if mode == Mode::Latency {
        ENTRY_CYCLE.store(cycle, Ordering::Relaxed);
    }
    log(ENTER, line);
    match mode {
        Mode::Nesting => {
            if (PRIMARY..PRIMARY + 7).contains(&line) {
                let level = line - PRIMARY + 1;
                pend(SIBLING + level - 1);
                if level > 1 {
                    pend(SIBLING + level - 2);
                }
                if level < 7 {
                    pend(line + 1);
                }
            }
        }
        Mode::FromHigher if line == HIGH => pend(LOW),
        Mode::TwiceFromHigher if line == HIGH => {
            pend(LOW);
            pend(LOW);
        }
        Mode::FromLower if line == LOW => pend(MIDDLE),
        Mode::OwnHandler if line == MIDDLE => {
            count(0);
            if LINE_ENTRIES[0].load(Ordering::Relaxed) == 1 {
                pend(MIDDLE);
            }
        }
        Mode::Neighbours => match line {
            TARGET => count(0),
            NEIGHBOUR_SOURCE => {
                count(1);
                if let Some(s) = source() {
                    s.store_hp_cpu_int_from_cpu_2(false);
                    let _ = s.read_hp_cpu_int_from_cpu_2();
                }
            }
            NEIGHBOUR_PENDING => count(2),
            _ => {}
        },
        Mode::Latency if line == HIGH => {
            let _ = masked(|clic| {
                pend_in(clic, LOW);
                PEND_CYCLE.store(oer_portable_riscv::cycle32(), Ordering::Relaxed);
            });
        }
        _ => {}
    }
    log(EXIT, line);
    true
}

/// Configures a line with MIE cleared and the session's own borrow: enable
/// off, then attributes and control, then the enable as asked.
fn configure(s: &mut Session<'_>, line: u32, level: usize, edge: bool, enable: bool) {
    let (Some(i), Some(&control)) = (index(line), LEVEL_CONTROL.get(level.wrapping_sub(1))) else {
        return;
    };
    let clic = &mut s.interrupt_matrix.clic[0];
    clic.modify_ie(i, |w| {
        w.set_ie(false);
    });
    clic.modify_ip(i, |w| {
        w.set_ip(false);
    });
    clic.modify_attr(i, |w| {
        w.set_shv(true).set_trig(if edge {
            pac::clic::attr::Trig::PositiveEdge
        } else {
            pac::clic::attr::Trig::PositiveLevel
        });
    });
    clic.modify_ctl(i, |w| {
        w.set_ctl(control);
    });
    if enable {
        clic.modify_ie(i, |w| {
            w.set_ie(true);
        });
    }
}

fn begin(s: &mut Session<'_>, mode: Mode) {
    oer_portable_riscv::disable_interrupts();
    EVENT_COUNT.store(0, Ordering::Relaxed);
    ENTRIES.store(0, Ordering::Relaxed);
    for c in &LINE_ENTRIES {
        c.store(0, Ordering::Relaxed);
    }
    CLIC.store(
        ptr::from_mut(&mut s.interrupt_matrix.clic[0]),
        Ordering::Release,
    );
    ACTIVE.store(mode as u32, Ordering::Release);
}

fn window() {
    let start = oer_portable_riscv::cycle32();
    while oer_portable_riscv::cycle32().wrapping_sub(start) < WINDOW {}
}

fn end() {
    oer_portable_riscv::disable_interrupts();
    ACTIVE.store(Mode::Idle as u32, Ordering::Release);
    CLIC.store(ptr::null_mut(), Ordering::Release);
}

/// Emits the trial's events: a header `(tag | 0xff, repeat, events,
/// entries)`, then six 16-bit events to a record:
/// `(tag | record << 16 | repeat << 24, e0 | e1 << 16, e2 | e3 << 16, e4 | e5 << 16)`.
fn emit_events(tag: u32, repeat: u32) {
    let n = (EVENT_COUNT.load(Ordering::Relaxed) as usize).min(EVENT_CAPACITY);
    emit(
        tag | 0xff,
        repeat,
        n as u32,
        ENTRIES.load(Ordering::Relaxed),
    );
    let event = |k: usize| {
        EVENTS
            .get(k)
            .filter(|_| k < n)
            .map_or(0xffff, |e| e.load(Ordering::Relaxed) & 0xffff)
    };
    let word = |k: usize| event(k) | event(k + 1) << 16;
    let mut k = 0;
    let mut record = 0u32;
    while k < n {
        emit(
            tag | record << 16 | repeat << 24,
            word(k),
            word(k + 2),
            word(k + 4),
        );
        k += 6;
        record += 1;
    }
}

fn pending(s: &Session<'_>, line: u32) -> u32 {
    index(line).map_or(0xff, |i| {
        u32::from(s.interrupt_matrix.clic[0].read_ip(i).ip())
    })
}

/// The line's described fields at their positions in the word: `IP` 0,
/// `IE` 8, `ATTR_SHV` 16, `ATTR_TRIG` 17-18, `ATTR_MODE` 22-23, `CTL` 24-31.
fn word(s: &Session<'_>, line: u32) -> u32 {
    index(line).map_or(0, |i| {
        let clic = &s.interrupt_matrix.clic[0];
        let attributes = clic.read_attr(i);
        u32::from(clic.read_ip(i).ip())
            | u32::from(clic.read_ie(i).ie()) << 8
            | u32::from(attributes.shv()) << 16
            | attributes.trig().get() << 17
            | attributes.mode().get() << 22
            | u32::from(clic.read_ctl(i).ctl()) << 24
    })
}

/// 310 (Q1): the thread pends level 1's primary line. The handler of level
/// n pends its same-level sibling, the sibling of level n-1, then level
/// n+1's primary line. Siblings record entry and exit only.
pub(super) fn nesting(s: &mut Session<'_>) {
    for repeat in 0..REPEATS {
        feed(s);
        setup(s);
        for n in 1..=7 {
            configure(s, PRIMARY + n as u32 - 1, n, true, true);
            configure(s, SIBLING + n as u32 - 1, n, true, true);
        }
        begin(s, Mode::Nesting);
        oer_portable_riscv::enable_interrupts();
        pend(PRIMARY);
        window();
        end();
        emit_events(0x3100, repeat);
    }
}

/// 311 (Q2.1): the thread pends HIGH (level 3); its handler pends LOW
/// (level 1). 316 (Q2.5, second half): the handler pends LOW twice.
fn from_higher(s: &mut Session<'_>, mode: Mode, tag: u32) {
    for repeat in 0..REPEATS {
        feed(s);
        setup(s);
        configure(s, LOW, 1, true, true);
        configure(s, HIGH, 3, true, true);
        begin(s, mode);
        oer_portable_riscv::enable_interrupts();
        pend(HIGH);
        window();
        end();
        emit_events(tag, repeat);
    }
}
pub(super) fn from_higher_once(s: &mut Session<'_>) {
    from_higher(s, Mode::FromHigher, 0x3200);
}

/// 312 (Q2.2): a pend of MIDDLE (level 2) from the thread (level 0), then
/// from LOW's handler (level 1).
pub(super) fn from_lower(s: &mut Session<'_>) {
    for repeat in 0..REPEATS {
        feed(s);
        setup(s);
        configure(s, LOW, 1, true, true);
        configure(s, MIDDLE, 2, true, true);
        begin(s, Mode::FromLower);
        oer_portable_riscv::enable_interrupts();
        pend(MIDDLE);
        log(MARK, 0);
        pend(LOW);
        window();
        end();
        emit_events(0x3300, repeat);
    }
}

/// 313 (Q2.3): MIDDLE's handler pends MIDDLE on its first entry.
pub(super) fn own_handler(s: &mut Session<'_>) {
    for repeat in 0..REPEATS {
        feed(s);
        setup(s);
        configure(s, MIDDLE, 2, true, true);
        begin(s, Mode::OwnHandler);
        oer_portable_riscv::enable_interrupts();
        pend(MIDDLE);
        window();
        end();
        emit_events(0x3400, repeat);
    }
}

/// 314 (Q2.4): NEIGHBOUR_PENDING (edge, pended, disabled), TARGET (edge,
/// enabled) and NEIGHBOUR_SOURCE (FROM_CPU2 routed, enabled; level in
/// variants 0-2, edge in 3-5), all level 2. With MIE cleared the source
/// rises before the target's pend (0, 3), between its read and its write
/// (1, 4) or after it (2, 5); then MIE is set. Record per trial:
/// `(0x3500 | variant, repeat, target | source << 8 | pending-line << 16
/// entries, words)`, then the three words after the pend.
pub(super) fn neighbours(s: &mut Session<'_>) {
    for variant in 0..6u32 {
        for repeat in 0..REPEATS {
            feed(s);
            setup(s);
            s.interrupt_matrix
                .interrupt_core0
                .modify_route(super::SOFTWARE_SOURCE, |w| {
                    w.set_map(Bits::truncate(NEIGHBOUR_SOURCE));
                });
            s.source.store_hp_cpu_int_from_cpu_2(false);
            let _ = s.source.read_hp_cpu_int_from_cpu_2();
            configure(s, NEIGHBOUR_PENDING, 2, true, false);
            configure(s, TARGET, 2, true, true);
            configure(s, NEIGHBOUR_SOURCE, 2, variant >= 3, true);
            if let Some(i) = index(NEIGHBOUR_PENDING) {
                s.interrupt_matrix.clic[0].modify_ip(i, |w| {
                    w.set_ip(true);
                });
            }
            let before = [
                word(s, NEIGHBOUR_PENDING),
                word(s, TARGET),
                word(s, NEIGHBOUR_SOURCE),
            ];
            let timing = variant % 3;
            if timing == 0 {
                raise_source();
            }
            if let Some(i) = index(TARGET) {
                s.interrupt_matrix.clic[0].modify_ip(i, |w| {
                    if timing == 1 {
                        raise_source();
                    }
                    w.set_ip(true);
                });
            }
            if timing == 2 {
                raise_source();
            }
            oer_portable_riscv::fence();
            let after = [
                word(s, NEIGHBOUR_PENDING),
                word(s, TARGET),
                word(s, NEIGHBOUR_SOURCE),
            ];
            begin(s, Mode::Neighbours);
            oer_portable_riscv::enable_interrupts();
            window();
            end();
            let counts = LINE_ENTRIES[0].load(Ordering::Relaxed)
                | LINE_ENTRIES[1].load(Ordering::Relaxed) << 8
                | LINE_ENTRIES[2].load(Ordering::Relaxed) << 16
                | (pending(s, TARGET)
                    | pending(s, NEIGHBOUR_SOURCE) << 1
                    | pending(s, NEIGHBOUR_PENDING) << 2)
                    << 24;
            emit(
                0x3500 | variant,
                repeat,
                counts,
                ENTRIES.load(Ordering::Relaxed),
            );
            if repeat == 0 {
                emit(0x3580 | variant, before[0], before[1], before[2]);
                emit(0x35c0 | variant, after[0], after[1], after[2]);
            }
        }
    }
    s.interrupt_matrix
        .interrupt_core0
        .modify_route(super::SOFTWARE_SOURCE, |w| {
            w.set_map(Bits::constant::<
                { pac::interrupt_core::route::Map::Disconnected.bits() },
            >());
        });
}

/// 315 (Q2.5): two pends of MIDDLE from the thread with MIE cleared, then
/// MIE set; then HIGH's handler pends LOW twice.
pub(super) fn twice(s: &mut Session<'_>) {
    for repeat in 0..REPEATS {
        feed(s);
        setup(s);
        configure(s, MIDDLE, 2, true, true);
        // Both pends before any entry: MIE stays cleared across them.
        if let Some(i) = index(MIDDLE) {
            for _ in 0..2 {
                s.interrupt_matrix.clic[0].modify_ip(i, |w| {
                    w.set_ip(true);
                });
            }
        }
        let ip = pending(s, MIDDLE);
        begin(s, Mode::OwnHandler);
        // OwnHandler pends again on its first entry only: start its count at one.
        LINE_ENTRIES[0].store(1, Ordering::Relaxed);
        oer_portable_riscv::enable_interrupts();
        window();
        end();
        emit(
            0x3600,
            repeat,
            ip,
            LINE_ENTRIES[0].load(Ordering::Relaxed).wrapping_sub(1),
        );
    }
    from_higher(s, Mode::TwiceFromHigher, 0x3700);
}

/// 316 (Q3): HIGH (level 3) pends LOW (level 1) and returns at once. The
/// cycle read right after the pend's write, to LOW's first cycle read in
/// the facts handler. Records `(0x3800, sample, delta, entries)`, then
/// `(0x38ff, min, max, samples)`.
pub(super) fn latency(s: &mut Session<'_>) {
    let mut min = u32::MAX;
    let mut max = 0u32;
    let mut valid = 0u32;
    for sample in 0..SAMPLES {
        feed(s);
        setup(s);
        configure(s, LOW, 1, true, true);
        configure(s, HIGH, 3, true, true);
        PEND_CYCLE.store(0, Ordering::Relaxed);
        ENTRY_CYCLE.store(0, Ordering::Relaxed);
        begin(s, Mode::Latency);
        oer_portable_riscv::enable_interrupts();
        let _ = masked(|clic| pend_in(clic, HIGH));
        window();
        end();
        let delta = ENTRY_CYCLE
            .load(Ordering::Relaxed)
            .wrapping_sub(PEND_CYCLE.load(Ordering::Relaxed));
        let entries = ENTRIES.load(Ordering::Relaxed);
        if entries == 2 {
            min = min.min(delta);
            max = max.max(delta);
            valid += 1;
        }
        if sample % 3 == 0 {
            emit(0x3800, sample, delta, entries);
        }
    }
    emit(0x38ff, min, max, valid);
}
