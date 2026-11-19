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
    if LEVEL_BITS != 3 || LINE + 32 > pac::clic::ctrl::COUNT {
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
    /// 317, 319, 320 (`tasks/219` c4, `tasks/241`): the pend as one byte
    /// store; the line counts its entries.
    BytePend = 8,
    /// 318: HIGH's handler stores the line's `IP` byte.
    ByteFromHigher = 9,
    /// 319: the line's handler stores its own `IP` byte on its first entry.
    ByteOwnHandler = 10,
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
        // `tasks/241`: 317's mode, absent here in run 233 and 234, made
        // every entry of the byte test the generic handler's.
        8 => Mode::BytePend,
        9 => Mode::ByteFromHigher,
        10 => Mode::ByteOwnHandler,
        _ => Mode::Idle,
    }
}

fn index(line: u32) -> Option<Index<{ pac::clic::ctrl::COUNT }>> {
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
        clic.modify_ctrl(i, |w| {
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
                clic.modify_ctrl(i, |w| {
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
        Mode::BytePend if line == BYTE_LINE => count(0),
        Mode::ByteFromHigher if line == HIGH => store_byte(BYTE_LINE, Byte::Ip, 1),
        Mode::ByteFromHigher if line == BYTE_LINE => count(0),
        Mode::ByteOwnHandler if line == BYTE_LINE => {
            count(0);
            if LINE_ENTRIES[0].load(Ordering::Relaxed) == 1 {
                store_byte(BYTE_LINE, Byte::Ip, 1);
            }
        }
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
    clic.modify_ctrl(i, |w| {
        w.set_ie(false);
    });
    clic.modify_ctrl(i, |w| {
        w.set_ip(false)
            .set_attr_shv(true)
            .set_attr_trig(if edge {
                pac::clic::ctrl::AttrTrig::PositiveEdge
            } else {
                pac::clic::ctrl::AttrTrig::PositiveLevel
            })
            .set_ctl(control);
    });
    if enable {
        clic.modify_ctrl(i, |w| {
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
        u32::from(s.interrupt_matrix.clic[0].read_ctrl(i).ip())
    })
}

/// The line's described fields at their positions in the word: `IP` 0,
/// `IE` 8, `ATTR_SHV` 16, `ATTR_TRIG` 17-18, `ATTR_MODE` 22-23, `CTL` 24-31.
fn word(s: &Session<'_>, line: u32) -> u32 {
    index(line).map_or(0, |i| {
        let w = s.interrupt_matrix.clic[0].read_ctrl(i);
        u32::from(w.ip())
            | u32::from(w.ie()) << 8
            | u32::from(w.attr_shv()) << 16
            | w.attr_trig().get() << 17
            | w.attr_mode().get() << 22
            | u32::from(w.ctl()) << 24
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
                s.interrupt_matrix.clic[0].modify_ctrl(i, |w| {
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
                s.interrupt_matrix.clic[0].modify_ctrl(i, |w| {
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
                s.interrupt_matrix.clic[0].modify_ctrl(i, |w| {
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

/// The CLIC's base on every hart (`chips/esp32s31/registers/chip.toml`,
/// instance `CLIC`) and its line words' offset (`types/clic.toml`, `CTRL%s`
/// at 0x1000, one word a line). Each word is four byte registers: `IP` at
/// +0, `IE` +1, `ATTR` +2, `CTL` +3. SOURCE: ESP-IDF 4d80c1b2
/// `components/soc/esp32s31/include/soc/clic_reg.h:113-121`
/// (`BYTE_CLIC_INT_*_REG(i)`, "each of following registers are 8bits").
const CLIC_BASE: usize = 0x1080_0000;
const CTRL_OFFSET: usize = 0x1000;
/// `tasks/241`: the byte tests' line, #1341's TARGET (34) between its
/// neighbours 33 and 35, all level 2 and edge-triggered, as 314 bound them.
const BYTE_LINE: u32 = TARGET;
/// A byte register of a line's word.
#[derive(Clone, Copy)]
enum Byte {
    Ip = 0,
    Ie = 1,
    Attr = 2,
    Ctl = 3,
}

/// One store of `value` to `byte` of `line`'s word, MIE as it is: no read
/// of the word, so no other byte is written (`tasks/219` c4, `tasks/241`).
#[allow(
    unsafe_code,
    reason = "probe-only byte store to a CLIC line's byte register"
)]
fn store_byte(line: u32, byte: Byte, value: u8) {
    let address = CLIC_BASE + CTRL_OFFSET + 4 * line as usize + byte as usize;
    // SAFETY: the hart's own CLIC line word's byte register, an MMIO byte
    // the vendor header names; this thread owns the CLIC for the session and
    // a handler that stores to it does so only in the trial's mode.
    unsafe { (address as *mut u8).write_volatile(value) };
}

/// The line's word as `word` reads it, through the loan with MIE cleared.
fn word_masked(line: u32) -> u32 {
    masked(|clic| {
        index(line).map_or(0, |i| {
            let w = clic.read_ctrl(i);
            u32::from(w.ip())
                | u32::from(w.ie()) << 8
                | u32::from(w.attr_shv()) << 16
                | w.attr_trig().get() << 17
                | w.attr_mode().get() << 22
                | u32::from(w.ctl()) << 24
        })
    })
    .unwrap_or(0)
}

/// The word's bits other than `IP`.
const NOT_IP: u32 = !1;
/// The trials of each byte test.
const BYTE_TRIALS: u32 = 100;

/// Wait until the line's entries pass `before`, within the window.
fn await_entry(before: u32) {
    let start = oer_portable_riscv::cycle32();
    while LINE_ENTRIES[0].load(Ordering::Relaxed) == before
        && oer_portable_riscv::cycle32().wrapping_sub(start) < WINDOW
    {}
}

/// One trial's start: the line bound as 314 binds TARGET (level 2, edge,
/// enabled), its pending bit cleared before the enable, and its word.
fn bind_byte_line(s: &mut Session<'_>) -> u32 {
    feed(s);
    setup(s);
    configure(s, BYTE_LINE, 2, true, false);
    if let Some(i) = index(BYTE_LINE) {
        s.interrupt_matrix.clic[0].modify_ctrl(i, |w| {
            w.set_ip(false);
        });
    }
    configure(s, BYTE_LINE, 2, true, true);
    word(s, BYTE_LINE)
}

/// One test's result: `(tag, trials that held, trials, the first that did
/// not: trial << 16 | what it saw)`.
fn result(tag: u32, held: u32, first_bad: u32) {
    emit(tag, held, BYTE_TRIALS, first_bad);
}

/// 317 (`tasks/241` 2a): a store of 1 to the `IP` byte from the thread
/// (level 0): the line is entered before the next step and once; the other
/// three bytes read back unchanged. Emits `(0x3800, held, trials, first
/// bad)`, then `(0x3801, entered before the next step, ...)`.
pub(super) fn byte_pend(s: &mut Session<'_>) {
    let (mut held, mut at_once, mut first_bad, mut first_late) = (0, 0, 0, 0);
    for trial in 0..BYTE_TRIALS {
        let before = bind_byte_line(s);
        begin(s, Mode::BytePend);
        oer_portable_riscv::enable_interrupts();
        store_byte(BYTE_LINE, Byte::Ip, 1);
        // The next step: the entry, if it came at the store.
        let next = LINE_ENTRIES[0].load(Ordering::Relaxed);
        await_entry(0);
        let entries = LINE_ENTRIES[0].load(Ordering::Relaxed);
        let after = word_masked(BYTE_LINE);
        end();
        let ok = entries == 1 && after & NOT_IP == before & NOT_IP && after & 1 == 0;
        held += u32::from(ok);
        if !ok && first_bad == 0 {
            first_bad = trial << 16 | entries << 8 | u32::from(after & NOT_IP != before & NOT_IP);
        }
        at_once += u32::from(next == 1);
        if next != 1 && first_late == 0 {
            first_late = trial << 16 | next;
        }
    }
    result(0x3800, held, first_bad);
    result(0x3801, at_once, first_late);
}

/// 318 (`tasks/241` 2b): HIGH (level 3) pended from the thread; its handler
/// stores the `IP` byte of the line (level 2), which is entered once, after
/// HIGH's return. Emits `(0x3900, held, trials, first bad)`.
pub(super) fn byte_from_higher(s: &mut Session<'_>) {
    let (mut held, mut first_bad) = (0, 0);
    for trial in 0..BYTE_TRIALS {
        bind_byte_line(s);
        configure(s, HIGH, 3, true, true);
        begin(s, Mode::ByteFromHigher);
        oer_portable_riscv::enable_interrupts();
        pend(HIGH);
        window();
        end();
        let order = entered_after_exit(HIGH, BYTE_LINE);
        let entries = LINE_ENTRIES[0].load(Ordering::Relaxed);
        let ok = entries == 1 && order;
        held += u32::from(ok);
        if !ok && first_bad == 0 {
            first_bad = trial << 16 | entries << 8 | u32::from(order);
        }
    }
    result(0x3900, held, first_bad);
}

/// Whether the log enters `line` once, after `before`'s exit.
fn entered_after_exit(before: u32, line: u32) -> bool {
    let n = (EVENT_COUNT.load(Ordering::Relaxed) as usize).min(EVENT_CAPACITY);
    let event = |k: usize| EVENTS.get(k).map_or(0, |e| e.load(Ordering::Relaxed));
    // `log`'s event: kind << 14 | line << 8 | level.
    let is = |k: usize, kind: u32, at: u32| event(k) >> 8 == (kind << 6 | (at & 0x3f));
    let exit = (0..n).position(|k| is(k, EXIT, before));
    let enter = (0..n).position(|k| is(k, ENTER, line));
    matches!((exit, enter), (Some(exit), Some(enter)) if enter > exit)
}

/// 319 (`tasks/241` 2c): two stores of the `IP` byte with MIE cleared
/// before the entry give one entry (`0x3a00`); a store in the line's own
/// handler, on its first entry, gives exactly one more (`0x3a01`).
pub(super) fn byte_twice_and_own(s: &mut Session<'_>) {
    let (mut held, mut first_bad) = (0, 0);
    for trial in 0..BYTE_TRIALS {
        bind_byte_line(s);
        begin(s, Mode::BytePend);
        store_byte(BYTE_LINE, Byte::Ip, 1);
        store_byte(BYTE_LINE, Byte::Ip, 1);
        oer_portable_riscv::enable_interrupts();
        await_entry(0);
        window();
        end();
        let entries = LINE_ENTRIES[0].load(Ordering::Relaxed);
        held += u32::from(entries == 1);
        if entries != 1 && first_bad == 0 {
            first_bad = trial << 16 | entries;
        }
    }
    result(0x3a00, held, first_bad);
    let (mut held, mut first_bad) = (0, 0);
    for trial in 0..BYTE_TRIALS {
        bind_byte_line(s);
        begin(s, Mode::ByteOwnHandler);
        oer_portable_riscv::enable_interrupts();
        store_byte(BYTE_LINE, Byte::Ip, 1);
        window();
        end();
        let entries = LINE_ENTRIES[0].load(Ordering::Relaxed);
        held += u32::from(entries == 2);
        if entries != 2 && first_bad == 0 {
            first_bad = trial << 16 | entries;
        }
    }
    result(0x3a01, held, first_bad);
}

/// 320 (`tasks/241` 2d): the neighbours 33 and 35 edge-triggered at level
/// 2, disabled and pending; a store of the line's `IP` byte enters the line
/// once and leaves both neighbours' words as they were. Emits `(0x3b00,
/// held, trials, first bad: trial << 16 | entries << 8 | 33 changed | 35
/// changed << 1)`.
pub(super) fn byte_neighbours(s: &mut Session<'_>) {
    let (mut held, mut first_bad) = (0, 0);
    for trial in 0..BYTE_TRIALS {
        bind_byte_line(s);
        for line in [NEIGHBOUR_PENDING, NEIGHBOUR_SOURCE] {
            configure(s, line, 2, true, false);
            if let Some(i) = index(line) {
                s.interrupt_matrix.clic[0].modify_ctrl(i, |w| {
                    w.set_ip(true);
                });
            }
        }
        let before = [word(s, NEIGHBOUR_PENDING), word(s, NEIGHBOUR_SOURCE)];
        begin(s, Mode::BytePend);
        oer_portable_riscv::enable_interrupts();
        store_byte(BYTE_LINE, Byte::Ip, 1);
        await_entry(0);
        let after = [
            word_masked(NEIGHBOUR_PENDING),
            word_masked(NEIGHBOUR_SOURCE),
        ];
        end();
        let entries = LINE_ENTRIES[0].load(Ordering::Relaxed);
        let changed = u32::from(after[0] != before[0]) | u32::from(after[1] != before[1]) << 1;
        let ok = entries == 1 && changed == 0;
        held += u32::from(ok);
        if !ok && first_bad == 0 {
            first_bad = trial << 16 | entries << 8 | changed;
        }
        // The neighbours' pending bits down again, the lines disabled.
        for line in [NEIGHBOUR_PENDING, NEIGHBOUR_SOURCE] {
            if let Some(i) = index(line) {
                s.interrupt_matrix.clic[0].modify_ctrl(i, |w| {
                    w.set_ip(false);
                });
            }
        }
    }
    result(0x3b00, held, first_bad);
}

/// 321 (`tasks/241` 3): with MIE cleared and the line disabled, a store to
/// each of `IE`, `ATTR` and `CTL` changes its own byte and no other: `CTL`
/// to another level's byte, `ATTR` to level-triggered, `IE` to one, each
/// read back and stored back. Emits `(0x3c00 | byte, held, trials, first
/// bad: trial << 16 | which fields changed)`.
pub(super) fn byte_fields(s: &mut Session<'_>) {
    for (byte, tag) in [
        (Byte::Ctl, 0x3c03),
        (Byte::Attr, 0x3c02),
        (Byte::Ie, 0x3c01),
    ] {
        let (mut held, mut first_bad) = (0, 0);
        for trial in 0..BYTE_TRIALS {
            feed(s);
            setup(s);
            configure(s, BYTE_LINE, 2, true, false);
            let before = word(s, BYTE_LINE);
            // What the store makes of its own byte, and the other bytes'
            // bits that must not change.
            let (value, own) = match byte {
                Byte::Ctl => (LEVEL_CONTROL.get(3).copied().unwrap_or(0), 0xff00_0000),
                // shv 1, edge cleared (positive level), mode 3.
                Byte::Attr => (0b1100_0001, 0x00ff_0000),
                Byte::Ie => (1, 0x0000_ff00),
                Byte::Ip => (0, 0x0000_00ff),
            };
            store_byte(BYTE_LINE, byte, value);
            let after = word(s, BYTE_LINE);
            let others = after & !own == before & !own;
            let own_changed = after & own != before & own;
            let ok = others && own_changed;
            held += u32::from(ok);
            if !ok && first_bad == 0 {
                first_bad = trial << 16 | u32::from(!others) | u32::from(!own_changed) << 1;
            }
            // Back to the bound line's byte.
            let restore = (before & own) >> own.trailing_zeros();
            store_byte(BYTE_LINE, byte, restore as u8);
        }
        result(tag, held, first_bad);
    }
}
