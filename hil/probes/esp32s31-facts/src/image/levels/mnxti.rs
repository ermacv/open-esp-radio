//! Selections 324-329 (tasks/255): the CLIC's next-interrupt CSR `mnxti`
//! (0x345) on hart 0, from the draft `riscv-fast-interrupt` v0.9
//! (`src/clic.adoc`, "Next Interrupt Handler Address and Interrupt-Enable
//! CSRs", "C-ABI Trampoline Code"), never ratified and used by no vendor
//! source.
//!
//! The lines are not hardware-vectored (`SHV` clear) but in 328, and are
//! positive-edge. They enter at mtvec's base, the common entry
//! (`_facts_start_trap`), which calls `facts_trap`; it finds the line in
//! `mcause`'s exception code (bits 11:0, an interrupt with bit 31 set) and
//! answers how the entry leaves. An exception while a selection runs is
//! recorded (`mcause`, `mepc`, `mtval`) and left past its instruction:
//! selection 324's "no". The installed table is `_facts_mnxti_mtvt`: the
//! chained line's word is the C-ABI function `facts_mnxti_chained`.
//!
//! The context line P (level 2), A (level 5), B (a level each case) and C
//! (level 3, the chained line) are four lines of the probe's range. A runs
//! with MIE cleared (the common entry does not set it), so nothing it pends
//! enters until it returns. Every handler clears its own pending byte
//! first: the common entry is not hardware-vectored, so the hardware does
//! not clear it (each handler records the byte it found).

use super::super::{
    FACTS_MNXTI_STAMP, Session, Table, emit, facts_mnxti_claim, facts_mnxti_read, setup_with,
};
use super::{
    LEVEL_CONTROL, Mode, PRIMARY, Vectoring, begin, clear_in, configure_as, end, feed, index,
    masked, pend_in, window,
};
use core::sync::atomic::{AtomicU32, Ordering};

/// The context line of 327, level 2.
const P_LINE: u32 = PRIMARY + 24;
/// The handler that reads or claims, level 5.
const A_LINE: u32 = PRIMARY + 25;
/// The line pended in A.
const B_LINE: u32 = PRIMARY + 26;
/// The chained line of 329, level 3: its word in `_facts_mnxti_mtvt` is
/// `facts_mnxti_chained` (`build.rs`'s `first + 27`).
const C_LINE: u32 = PRIMARY + 27;
const P_LEVEL: usize = 2;
const A_LEVEL: usize = 5;
const C_LEVEL: usize = 3;
const _: () = if C_LINE as usize >= oer_chips_esp32s31_pac::clic::ip::COUNT {
    panic!("the mnxti lines lie in the CLIC");
};

/// Trials of a case.
const TRIALS: u32 = 100;
/// A storm's bound: entries of the common entry in one trial.
const ENTRY_LIMIT: u32 = 16;
/// A context handler's wait for A's entry, in cycles.
const NESTED_CYCLES: u32 = 20_000;

/// The common entry leaves through the exit (`1`), past the trapping
/// instruction (`3`) or through the claim of the next interrupt (`2`).
const LEAVE: u32 = 1;
const CHAIN: u32 = 2;
const SKIP: u32 = 3;

/// What A does, and whether an exception is expected.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum Step {
    Idle = 0,
    /// 324: only the thread reads; an exception is recorded and skipped.
    Present = 1,
    /// 325: pend B, read without a claim.
    Read = 2,
    /// 326: pend B, claim.
    Claim = 3,
    /// 327: entered from P's handler; pend B, read (with a raised threshold
    /// when `THRESHOLD` is set).
    Qualify = 4,
    /// 328: pend the vectored B, read.
    Vectored = 5,
    /// 329: pend C, leave through the exit.
    Return = 6,
    /// 329: pend C, leave through the claim.
    Chain = 7,
}

static STEP: AtomicU32 = AtomicU32::new(Step::Idle as u32);
/// 327's threshold in A, a level's control byte; zero for none.
static THRESHOLD: AtomicU32 = AtomicU32::new(0);
static ENTRIES: AtomicU32 = AtomicU32::new(0);
static A_ENTRIES: AtomicU32 = AtomicU32::new(0);
static C_ENTRIES: AtomicU32 = AtomicU32::new(0);
static CHAINED: AtomicU32 = AtomicU32::new(0);
static STRAYS: AtomicU32 = AtomicU32::new(0);
/// Cycles from the stamp before the exit to C's first statement.
static SPAN: AtomicU32 = AtomicU32::new(0);
/// The pending byte A found its own line's at its entry.
static A_FOUND: AtomicU32 = AtomicU32::new(0);
static TRAPS: AtomicU32 = AtomicU32::new(0);
static TRAP_CAUSE: AtomicU32 = AtomicU32::new(0);
static TRAP_PC: AtomicU32 = AtomicU32::new(0);
static TRAP_VALUE: AtomicU32 = AtomicU32::new(0);
/// A's observations, by step (see each selection).
static SEEN: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];

fn step() -> Step {
    match STEP.load(Ordering::Acquire) {
        1 => Step::Present,
        2 => Step::Read,
        3 => Step::Claim,
        4 => Step::Qualify,
        5 => Step::Vectored,
        6 => Step::Return,
        7 => Step::Chain,
        _ => Step::Idle,
    }
}

fn see(slot: usize, value: u32) {
    if let Some(word) = SEEN.get(slot) {
        word.store(value, Ordering::Relaxed);
    }
}

fn seen(slot: usize) -> u32 {
    SEEN.get(slot)
        .map_or(0, |word| word.load(Ordering::Relaxed))
}

fn control(level: usize) -> u32 {
    LEVEL_CONTROL
        .get(level.wrapping_sub(1))
        .map_or(0, |&control| u32::from(control))
}

/// A line's pending bit through the loan.
fn pending(line: u32) -> u32 {
    masked(|matrix| index(line).map_or(0xff, |i| u32::from(matrix.clic[0].read_ip(i).ip())))
        .unwrap_or(0xfe)
}

fn pend(line: u32) {
    let _ = masked(|matrix| pend_in(matrix, line));
}

fn clear(line: u32) {
    let _ = masked(|matrix| clear_in(matrix, line));
}

fn mcause() -> u32 {
    oer_portable_riscv::read_mcause() as u32
}

fn mintstatus() -> u32 {
    oer_portable_riscv::read_mintstatus() as u32
}

/// The common entry's call: how to leave, or `None` for a fault the probe
/// records and halts on (an interrupt or exception no selection expects).
pub(in super::super) fn trap(cycle: u32) -> Option<u32> {
    let cause = mcause();
    let step = step();
    if step == Step::Idle {
        return None;
    }
    if cause & (1 << 31) == 0 {
        TRAPS.fetch_add(1, Ordering::Relaxed);
        TRAP_CAUSE.store(cause, Ordering::Relaxed);
        TRAP_PC.store(oer_portable_riscv::read_mepc() as u32, Ordering::Relaxed);
        TRAP_VALUE.store(oer_portable_riscv::read_mtval() as u32, Ordering::Relaxed);
        return Some(SKIP);
    }
    // SOURCE: RISC-V CLIC specification 3f8ee744, mcause.exccode[11:0].
    let line = cause & 0xfff;
    if line == C_LINE {
        let stamp = FACTS_MNXTI_STAMP.load(Ordering::Relaxed);
        SPAN.store(cycle.wrapping_sub(stamp), Ordering::Relaxed);
        C_ENTRIES.fetch_add(1, Ordering::Relaxed);
        clear(C_LINE);
        return Some(LEAVE);
    }
    if ENTRIES.fetch_add(1, Ordering::Relaxed) >= ENTRY_LIMIT {
        STRAYS.fetch_add(1, Ordering::Relaxed);
        let _ = masked(|matrix| {
            if let Some(i) = index(line) {
                matrix.clic[0].modify_ie(i, |w| {
                    w.set_ie(false);
                });
            }
            clear_in(matrix, line);
        });
        return Some(LEAVE);
    }
    match line {
        P_LINE => {
            clear(P_LINE);
            pend(A_LINE);
            // A, above this level, preempts once MIE is set.
            oer_portable_riscv::enable_interrupts();
            let start = oer_portable_riscv::cycle32();
            loop {
                if A_ENTRIES.load(Ordering::Relaxed) != 0
                    || oer_portable_riscv::cycle32().wrapping_sub(start) >= NESTED_CYCLES
                {
                    break;
                }
            }
            oer_portable_riscv::disable_interrupts();
            Some(LEAVE)
        }
        A_LINE => {
            A_FOUND.store(pending(A_LINE), Ordering::Relaxed);
            clear(A_LINE);
            A_ENTRIES.fetch_add(1, Ordering::Relaxed);
            Some(handler_a(step))
        }
        _ => {
            STRAYS.fetch_add(1, Ordering::Relaxed);
            clear(line);
            Some(LEAVE)
        }
    }
}

/// A's step. MIE is clear throughout but after 326's claim.
fn handler_a(step: Step) -> u32 {
    match step {
        Step::Read => {
            pend(B_LINE);
            let (cause, status) = (mcause(), mintstatus());
            let value = facts_mnxti_read();
            see(0, value);
            see(1, cause);
            see(2, status);
            see(3, mcause());
            see(4, mintstatus());
            see(5, pending(B_LINE));
            clear(B_LINE);
            LEAVE
        }
        Step::Claim => {
            pend(B_LINE);
            let claimed = facts_mnxti_claim();
            let (cause, status) = (mcause(), mintstatus());
            oer_portable_riscv::disable_interrupts();
            see(0, claimed as u32);
            see(1, (claimed >> 32) as u32);
            see(2, cause);
            see(3, status);
            see(4, pending(B_LINE));
            clear(B_LINE);
            LEAVE
        }
        Step::Qualify => {
            pend(B_LINE);
            let threshold = THRESHOLD.load(Ordering::Relaxed) as u8;
            let value = if threshold == 0 {
                facts_mnxti_read()
            } else {
                let previous = oer_portable_riscv::swap_interrupt_threshold(threshold);
                let value = facts_mnxti_read();
                // Read the threshold back by storing the same value again.
                let held = oer_portable_riscv::swap_interrupt_threshold(threshold);
                see(4, u32::from(held));
                oer_portable_riscv::restore_interrupt_threshold(previous);
                value
            };
            see(0, value);
            see(1, mcause());
            see(2, mintstatus());
            see(3, pending(B_LINE));
            clear(B_LINE);
            LEAVE
        }
        Step::Vectored => {
            pend(B_LINE);
            see(0, facts_mnxti_read());
            see(1, pending(B_LINE));
            clear(B_LINE);
            LEAVE
        }
        Step::Return => {
            pend(C_LINE);
            LEAVE
        }
        Step::Chain => {
            pend(C_LINE);
            CHAIN
        }
        Step::Idle | Step::Present => LEAVE,
    }
}

/// The chained line's function, called by 329's claim inside A.
pub(in super::super) fn chained(cycle: u32) {
    let stamp = FACTS_MNXTI_STAMP.load(Ordering::Relaxed);
    SPAN.store(cycle.wrapping_sub(stamp), Ordering::Relaxed);
    CHAINED.fetch_add(1, Ordering::Relaxed);
}

fn reset() {
    for word in [
        &ENTRIES,
        &A_ENTRIES,
        &C_ENTRIES,
        &CHAINED,
        &STRAYS,
        &SPAN,
        &A_FOUND,
        &TRAPS,
        &TRAP_CAUSE,
        &TRAP_PC,
        &TRAP_VALUE,
    ] {
        word.store(0, Ordering::Relaxed);
    }
    for word in &SEEN {
        word.store(0, Ordering::Relaxed);
    }
}

/// One trial: the four lines set up (B at `b_level`, entering as
/// `vectoring`), A's step, then the thread pends A (or P in 327) with MIE
/// set and waits the window.
fn trial(s: &mut Session<'_>, step: Step, b_level: usize, vectoring: Vectoring) {
    feed(s);
    setup_with(s, Table::Mnxti);
    configure_as(s, P_LINE, P_LEVEL, true, true, Vectoring::Common);
    configure_as(s, A_LINE, A_LEVEL, true, true, Vectoring::Common);
    configure_as(s, B_LINE, b_level, true, true, vectoring);
    configure_as(s, C_LINE, C_LEVEL, true, true, Vectoring::Common);
    reset();
    begin(s, Mode::Idle);
    STEP.store(step as u32, Ordering::Release);
    oer_portable_riscv::enable_interrupts();
    pend(if step == Step::Qualify {
        P_LINE
    } else {
        A_LINE
    });
    window();
    end();
    STEP.store(Step::Idle as u32, Ordering::Release);
}

fn mtvt() -> u32 {
    oer_portable_riscv::read_mtvt() as u32
}

/// The table word `mnxti` names for `line`, by the draft (`mtvt + 4 × id`).
fn word_of(line: u32) -> u32 {
    (mtvt() & !0x3f).wrapping_add(4 * line)
}

/// 324: `mnxti` read from the thread, MIE clear and nothing pending, a
/// hundred times. `(0x3b00, reads, zeros, traps)`, `(0x3b01, mcause, mepc,
/// mtval)` of the last trap, `(0x3b02, last value, mtvt, 0)`.
pub(in super::super) fn present(s: &mut Session<'_>) {
    feed(s);
    setup_with(s, Table::Mnxti);
    reset();
    STEP.store(Step::Present as u32, Ordering::Release);
    let mut zeros = 0u32;
    let mut last = 0u32;
    for _ in 0..TRIALS {
        last = facts_mnxti_read();
        if last == 0 {
            zeros += 1;
        }
    }
    STEP.store(Step::Idle as u32, Ordering::Release);
    emit(0x3b00, TRIALS, zeros, TRAPS.load(Ordering::Relaxed));
    emit(
        0x3b01,
        TRAP_CAUSE.load(Ordering::Relaxed),
        TRAP_PC.load(Ordering::Relaxed),
        TRAP_VALUE.load(Ordering::Relaxed),
    );
    emit(0x3b02, last, mtvt(), 0);
}

/// The trial's common words: `(tag | 0x80, A's entries, the byte A found,
/// traps | strays << 16)`.
fn emit_common(tag: u32) {
    emit(
        tag | 0x80,
        A_ENTRIES.load(Ordering::Relaxed),
        A_FOUND.load(Ordering::Relaxed),
        TRAPS.load(Ordering::Relaxed) | STRAYS.load(Ordering::Relaxed) << 16,
    );
}

/// 325: in A, B (level 3) pended, `mnxti` read without a claim. Holds with
/// the value B's table word, `mcause` and `mintstatus` unchanged across the
/// read and B still pending. `(0x3c00, trials, held, last value)`,
/// `(0x3c01, B's word, mcause, mintstatus)` before the read, `(0x3c02,
/// mcause, mintstatus, B pending)` after it, then the common words.
pub(in super::super) fn read_only(s: &mut Session<'_>) {
    let mut held = 0u32;
    for _ in 0..TRIALS {
        trial(s, Step::Read, 3, Vectoring::Common);
        if A_ENTRIES.load(Ordering::Relaxed) == 1
            && seen(0) == word_of(B_LINE)
            && seen(1) == seen(3)
            && seen(2) == seen(4)
            && seen(5) == 1
        {
            held += 1;
        }
    }
    emit(0x3c00, TRIALS, held, seen(0));
    emit(0x3c01, word_of(B_LINE), seen(1), seen(2));
    emit(0x3c02, seen(3), seen(4), seen(5));
    emit_common(0x3c00);
}

/// 326: in A, B (level 3) pended, then the claim (`csrrsi mnxti, MIE`).
/// Holds with the value B's table word, B's pending bit clear after it,
/// `mcause`'s code B's, `mintstatus`'s level B's and MIE set.
/// `(0x3d00, trials, held, last value)`, `(0x3d01, mstatus, mcause,
/// mintstatus)` after the claim, `(0x3d02, B pending, B's word, B's
/// control byte)`, then the common words.
pub(in super::super) fn claim(s: &mut Session<'_>) {
    let mut held = 0u32;
    for _ in 0..TRIALS {
        trial(s, Step::Claim, 3, Vectoring::Common);
        if A_ENTRIES.load(Ordering::Relaxed) == 1
            && seen(0) == word_of(B_LINE)
            && seen(1) & oer_portable_riscv::machine::status::MIE as u32 != 0
            && seen(2) & 0xfff == B_LINE
            && seen(3) >> 24 == control(3)
            && seen(4) == 0
        {
            held += 1;
        }
    }
    emit(0x3d00, TRIALS, held, seen(0));
    emit(0x3d01, seen(1), seen(2), seen(3));
    emit(0x3d02, seen(4), word_of(B_LINE), control(3));
    emit_common(0x3d00);
}

/// 327: A entered from P's handler (level 2), B pended at a level a case,
/// `mnxti` read. Cases `(B's level, threshold, B qualifies)`: 3 (below A,
/// above P), 4, 6 (above A), 2 (P's), 1, and 3 with `mintthresh` at level
/// 4's control byte. Holds with A entered once from P (`mcause.mpil` P's
/// control byte) and a value of B's word when B qualifies, else zero.
/// `(0x3e00 | case, trials, held, last value)`, `(0x3e10 | case, mcause,
/// mintstatus, B pending | the threshold read back << 8)`, then the common
/// words at `0x3e20 | case`.
pub(in super::super) fn qualifies(s: &mut Session<'_>) {
    const CASES: [(usize, usize, bool); 6] = [
        (3, 0, true),
        (4, 0, true),
        (6, 0, true),
        (2, 0, false),
        (1, 0, false),
        (3, 4, false),
    ];
    for (case, &(level, threshold, qualifies)) in CASES.iter().enumerate() {
        let case = case as u32;
        THRESHOLD.store(
            if threshold == 0 {
                0
            } else {
                control(threshold)
            },
            Ordering::Relaxed,
        );
        let mut held = 0u32;
        for _ in 0..TRIALS {
            trial(s, Step::Qualify, level, Vectoring::Common);
            let expected = if qualifies { word_of(B_LINE) } else { 0 };
            if A_ENTRIES.load(Ordering::Relaxed) == 1
                && (seen(1) >> 16) & 0xff == control(P_LEVEL)
                && seen(0) == expected
            {
                held += 1;
            }
        }
        THRESHOLD.store(0, Ordering::Relaxed);
        emit(0x3e00 | case, TRIALS, held, seen(0));
        emit(0x3e10 | case, seen(1), seen(2), seen(3) | seen(4) << 8);
        emit_common(0x3e20 | case);
    }
}

/// 328: in A, a hardware-vectored B (level 3) pended, `mnxti` read.
/// `(0x3f00, trials, zeros, last value)`, `(0x3f01, B's word, B pending, 0)`,
/// then the common words.
pub(in super::super) fn vectored(s: &mut Session<'_>) {
    let mut zeros = 0u32;
    for _ in 0..TRIALS {
        trial(s, Step::Vectored, 3, Vectoring::Hardware);
        if A_ENTRIES.load(Ordering::Relaxed) == 1 && seen(0) == 0 {
            zeros += 1;
        }
    }
    emit(0x3f00, TRIALS, zeros, seen(0));
    emit(0x3f01, word_of(B_LINE), seen(1), 0);
    emit_common(0x3f00);
}

/// 329: A pends C (level 3), then leaves. Variant 0 through the exit: A's
/// frame restored, `mret`, the hardware enters C at the common entry, whose
/// frame is saved before `facts_trap`'s first statement. Variant 1 through
/// the claim: `csrrsi mnxti, MIE`, the load of C's table word and the jump
/// to `facts_mnxti_chained`, in A's frame. Both from the stamp before the
/// exit sequence. Holds with C entered once (0) or chained once and never
/// entered (1). `(0x4000 | variant, trials, held, 0)`, `(0x4010 | variant,
/// least, most, last)` in cycles, then the common words.
pub(in super::super) fn saving(s: &mut Session<'_>) {
    for (variant, step) in [(0u32, Step::Return), (1, Step::Chain)] {
        let mut held = 0u32;
        let mut least = u32::MAX;
        let mut most = 0u32;
        let mut last = 0u32;
        for _ in 0..TRIALS {
            trial(s, step, 3, Vectoring::Common);
            let (entered, chained) = (
                C_ENTRIES.load(Ordering::Relaxed),
                CHAINED.load(Ordering::Relaxed),
            );
            let ok = A_ENTRIES.load(Ordering::Relaxed) == 1
                && match step {
                    Step::Return => entered == 1 && chained == 0,
                    _ => entered == 0 && chained == 1,
                };
            last = SPAN.load(Ordering::Relaxed);
            if ok {
                held += 1;
                least = least.min(last);
                most = most.max(last);
            }
        }
        emit(0x4000 | variant, TRIALS, held, 0);
        emit(0x4010 | variant, least, most, last);
        emit_common(0x4020 | variant);
    }
}
