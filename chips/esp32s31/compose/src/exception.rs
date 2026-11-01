//! A CPU exception's interrupt observations, retained for the next boot.
//!
//! The runtime's exception path (`oer_espressif_staged_runtime::trap::handle`)
//! retains the trap frame first, before it stops the other hart: `mcause`,
//! `mepc`, `mtval`, `ra`, `sp` and `mscratch` as the trap left them, which
//! the next boot reports as `fatal_exception`. [`capture`], the image's
//! exception hook, runs after that stop and adds what the trap frame cannot
//! hold: the hart's pending sources, its trap vector registers and, when the
//! trap is an interrupt of a line without a handler, the sources routed to
//! that line and the line's state. These are read after the trap, under the
//! stop, never at the trap's instant: a source may have become pending, or
//! a route changed, in between. The record states whether the stop held;
//! without it, routes and the line are not read.
//!
//! Every step is a register read through a typed accessor or a bounded
//! copy, without allocation, waiting or a panic path, so the hook stays a
//! short leaf of the fatal path. The next boot's [`report_previous`] prints
//! the record and clears it only after every note was written whole.

#[cfg(any(test, target_arch = "riscv32"))]
use oer_diagnostics::Note;
use oer_diagnostics::fault::Record;
#[cfg(any(test, target_arch = "riscv32"))]
use oer_diagnostics::fault::{Class, Domain};
#[cfg(target_arch = "riscv32")]
use oer_espressif_staged_runtime::retained_fault;

/// The interrupt matrix's sources, in 32-bit words.
pub const SOURCE_WORDS: usize = 6;

#[cfg(any(test, target_arch = "riscv32"))]
/// The record's reason in its domain.
const OBSERVED: u32 = 1;
#[cfg(any(test, target_arch = "riscv32"))]
/// [`Payload::flags`]: the other hart was stopped for the reads.
const STOPPED: u32 = 1 << 0;
#[cfg(any(test, target_arch = "riscv32"))]
/// [`Payload::flags`]: the trap is an interrupt and `vector` is its line.
const INTERRUPT: u32 = 1 << 1;

oer_espressif_staged_runtime::retained_static! {
    /// The exception's observations. The fatal path takes it without
    /// waiting: while the other hart records its own failure, this one's is
    /// skipped.
    static SLOT: Record;
}

/// What the exception hook read after the trap.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Observed {
    /// The hart that trapped.
    pub hart: u32,
    /// Whether the fatal path stopped the other hart before the reads.
    pub stopped: bool,
    /// `mtvec` and `mtvt` as read.
    pub mtvec: u32,
    pub mtvt: u32,
    /// The sources pending on this hart, one bit each.
    pub pending: [u32; SOURCE_WORDS],
    /// For an interrupt of a line without a handler: the line and what was
    /// routed to it.
    pub line: Option<LineView>,
}

/// An interrupt line as read after the trap.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LineView {
    /// The line's number (`mcause`'s code).
    pub vector: u16,
    /// The handler address `mtvt` holds for it.
    pub slot: u32,
    /// Bit 0 pending, 1 enabled, 2 edge-triggered, bits 8.. its level
    /// byte; zero when nothing is routed to the line or the stop failed.
    pub state: u32,
    /// The sources routed to the line on this hart.
    pub routed: [u32; SOURCE_WORDS],
}

#[cfg(any(test, target_arch = "riscv32"))]
/// The retained words, in order.
#[derive(Clone, Copy, bytemuck::AnyBitPattern, bytemuck::NoUninit)]
#[repr(C)]
struct Payload {
    flags: u32,
    mtvec: u32,
    mtvt: u32,
    vector: u32,
    slot: u32,
    state: u32,
    pending: [u32; SOURCE_WORDS],
    routed: [u32; SOURCE_WORDS],
}

#[cfg(any(test, target_arch = "riscv32"))]
impl Observed {
    fn payload(&self) -> Payload {
        let line = self.line.unwrap_or_default();
        Payload {
            flags: (if self.stopped { STOPPED } else { 0 })
                | (if self.line.is_some() { INTERRUPT } else { 0 }),
            mtvec: self.mtvec,
            mtvt: self.mtvt,
            vector: u32::from(line.vector),
            slot: line.slot,
            state: line.state,
            pending: self.pending,
            routed: line.routed,
        }
    }

    /// The observations a record holds; `None` for a record of another
    /// domain or shape, or with unknown flags.
    fn from_record(view: &oer_diagnostics::fault::View<'_>) -> Option<Self> {
        if view.class != Class::CpuException || view.reason != OBSERVED {
            return None;
        }
        let payload: Payload = view.decode(Domain::ExceptionObservation).ok()?;
        if payload.flags & !(STOPPED | INTERRUPT) != 0 {
            return None;
        }
        let line = if payload.flags & INTERRUPT != 0 {
            Some(LineView {
                vector: u16::try_from(payload.vector).ok()?,
                slot: payload.slot,
                state: payload.state,
                routed: payload.routed,
            })
        } else {
            None
        };
        Some(Self {
            hart: view.hart,
            stopped: payload.flags & STOPPED != 0,
            mtvec: payload.mtvec,
            mtvt: payload.mtvt,
            pending: payload.pending,
            line,
        })
    }

    fn record(&self) -> Result<Record, oer_diagnostics::fault::Error> {
        Record::encode(
            Class::CpuException,
            Domain::ExceptionObservation,
            OBSERVED,
            self.hart,
            0,
            &self.payload(),
        )
    }
}

/// The image's exception hook: read the observations and commit them under
/// `claim`; the runtime's fatal reset follows.
#[cfg(target_arch = "riscv32")]
pub fn capture(
    claim: &oer_espressif_staged_runtime::retained_fault::Claim<'_>,
    frame: &oer_espressif_staged_runtime::trap::TrapFrame,
    stop: Result<&oer_esp32s31_hal::cores::StopTheWorld<'_>, oer_esp32s31_hal::cores::StopError>,
) {
    if let Ok(record) = observe(frame, stop).record() {
        let _committed = claim.commit(&SLOT, record);
    }
}

#[cfg(target_arch = "riscv32")]
fn observe(
    frame: &oer_espressif_staged_runtime::trap::TrapFrame,
    stop: Result<&oer_esp32s31_hal::cores::StopTheWorld<'_>, oer_esp32s31_hal::cores::StopError>,
) -> Observed {
    use oer_esp32s31_hal::platform::interrupt_controller;
    use oer_espressif_staged_runtime::trap::{self, TrapRegisters};

    let registers = TrapRegisters::read();
    let mut pending = [0; SOURCE_WORDS];
    // An invalid raw ID is retained by the runtime's fatal path; it grants
    // no other hart's bank for this diagnostic sample.
    let hart = oer_esp32s31_hal::platform::Hart::new(registers.hart as u32);
    for source in 0..SOURCE_WORDS * 32 {
        // At most 192 sources: the count fits a source number.
        if hart.is_ok_and(|hart| interrupt_controller::is_pending(hart, source as u16)) {
            set(&mut pending, source);
        }
    }
    // The trap frame's cause: an interrupt reaches this hook only from a
    // line the table never vectors.
    let mcause = frame.mcause as usize;
    let interrupt = mcause & (1 << 31) != 0;
    let line = interrupt.then(|| {
        let vector = mcause & 0x0fff;
        let mut view = LineView {
            vector: vector as u16,
            slot: trap::current_hart_vector_slot(vector).unwrap_or(0),
            ..LineView::default()
        };
        if let Ok(stop) = stop {
            let _observed = interrupt_controller::fatal_view(stop, |controller| {
                for source in 0..SOURCE_WORDS * 32 {
                    if controller.routed(source as u16).map(usize::from) == Some(vector) {
                        set(&mut view.routed, source);
                    }
                }
                if view.routed != [0; SOURCE_WORDS]
                    && let Some(observed) = u8::try_from(vector)
                        .ok()
                        .and_then(|vector| controller.line(vector))
                {
                    view.state = line_word(observed);
                }
            });
        }
        view
    });
    Observed {
        hart: registers.hart as u32,
        stopped: stop.is_ok(),
        mtvec: registers.mtvec as u32,
        mtvt: registers.mtvt as u32,
        pending,
        line,
    }
}

/// The record's word of an observed line: bit 0 pending, 1 enabled, 2
/// edge-triggered, bits 8.. its level byte.
#[cfg(target_arch = "riscv32")]
fn line_word(observed: oer_esp32s31_hal::platform::interrupt_controller::LineObservation) -> u32 {
    use oer_riscv::clic::Trigger;
    let oer_esp32s31_hal::platform::interrupt_controller::LineObservation { pending, state } =
        observed;
    u32::from(pending)
        | u32::from(state.enabled) << 1
        | u32::from(matches!(
            state.trigger,
            Trigger::PositiveEdge | Trigger::NegativeEdge
        )) << 2
        | u32::from(state.control.bits()) << 8
}

/// Set `source`'s bit, without an index whose bounds check could fail: a
/// panic while recording would re-enter the fatal path.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
fn set(words: &mut [u32; SOURCE_WORDS], source: usize) {
    if let Some(word) = words.get_mut(source / 32) {
        *word |= 1 << (source % 32);
    }
}

/// The previous boot's observations, without printing or clearing them:
/// read before [`report_previous`] so that the boot can serve them.
#[cfg(target_arch = "riscv32")]
pub fn peek_previous() -> Option<Observed> {
    let slot = retained_fault::peek(&SLOT)?;
    Observed::from_record(&slot.peek().ok()?)
}

/// Print the previous boot's observations early at boot; clear them only
/// after every note was written whole.
#[cfg(target_arch = "riscv32")]
pub fn report_previous() {
    if let Some(mut slot) = retained_fault::peek(&SLOT) {
        let _reported = report(&mut slot, |note| {
            crate::console::early(note)
                .is_whole()
                .then_some(())
                .ok_or(())
        });
    }
}

/// Print the task stack's watchpoint, which the runtime's handoff armed on
/// the stack's bottom word. The HAL reserves watchpoint zero for stack
/// monitoring and respects a debugger's ownership.
#[cfg(target_arch = "riscv32")]
pub fn report_stack_guard() {
    let (control, address) = oer_riscv::selected_trigger_data();
    let hart = oer_espressif_staged_runtime::trap::TrapRegisters::read().hart;
    let _written = crate::console::early(&Note::stack_guard(
        hart as u32,
        control as u32,
        address as u32,
    ));
}

/// Report `record` through `emit`; it stays until every note was written.
/// A record of another shape is neither printed nor cleared.
#[cfg(any(test, target_arch = "riscv32"))]
fn report(
    record: &mut Record,
    mut emit: impl FnMut(&Note<'_>) -> Result<(), ()>,
) -> Result<(), ()> {
    record
        .report(|view| {
            let observed = Observed::from_record(&view).ok_or(())?;
            // An exception names no line: only an interrupt has one.
            if let Some(line) = observed.line {
                emit(&Note::fatal_irq_vector(
                    u32::from(line.vector),
                    line.state,
                    observed.mtvec,
                    observed.mtvt,
                    line.slot,
                ))?;
            }
            let [p0, p1, p2, p3, p4, p5] = observed.pending;
            emit(&Note::fatal_irq_pending(p0, p1, p2, p3, p4, p5))?;
            // Routes are read only for an interrupt and only under the stop.
            if let Some(line) = observed.line.filter(|_| observed.stopped) {
                let [r0, r1, r2, r3, r4, r5] = line.routed;
                emit(&Note::fatal_irq_routed(r0, r1, r2, r3, r4, r5))?;
            }
            Ok::<(), ()>(())
        })
        .map_err(|_| ())
}

#[cfg(test)]
mod tests;
