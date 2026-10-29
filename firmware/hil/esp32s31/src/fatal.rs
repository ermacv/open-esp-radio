//! A fatal error's machine state, kept for the next boot.
//!
//! The panic hook and the exception entry record the hart's trap registers,
//! the vector it last took, the sources pending and routed to that vector and
//! its CLIC line in `.rtc_fast.persistent` without formatting: every step is
//! a register read through a typed accessor or a bounded copy, so the fatal
//! path stays a short leaf in every context's stack bound. The routes and the
//! line are read as the hardware holds them, under the fatal path's stop of
//! the other hart (`interrupt_controller::fatal_view`); without the stop
//! they read as routed nowhere. The chip then
//! resets, and the next boot's [`report_previous`] prints the record once.
//! A failed boot check whose note the console's driver kept off the USB
//! block records only its [`Failure`] and the hart, then resets the chip.

use oer_diagnostics::Note;
use oer_espressif_staged_runtime::retained_fault::{self, Claim};
use oer_diagnostics::fault::{Class, Domain, Record};
use oer_esp32s31_hal::cores::{StopError, StopTheWorld};
use oer_esp32s31_hal::platform::interrupt_controller::{self, LineObservation};
use oer_espressif_staged_runtime::trap::{self, TrapRegisters};
use oer_riscv::clic::Trigger;

#[cfg(feature = "panic-hook")]
const PANIC: u32 = 1;
const EXCEPTION: u32 = 2;
const FAILURE: u32 = 3;
/// The interrupt matrix's sources, in 32-bit words.
const SOURCE_WORDS: usize = 6;

/// Numeric source observations inside the common retained envelope.
#[derive(Clone, Copy, bytemuck::AnyBitPattern)]
#[repr(C)]
struct Payload {
    kind: u32,
    hart: u32,
    mcause: u32,
    mepc: u32,
    mtval: u32,
    ra: u32,
    mscratch: u32,
    mtvec: u32,
    mtvt: u32,
    /// The handler MTVT holds for the vector in `mcause`.
    mtvt_slot: u32,
    /// The image's diagnostic stage, for a panic; the [`Failure`], for a
    /// failed boot check.
    stage: u32,
    /// The vector's CLIC line: bit 0 pending, 1 enabled, 2 edge-triggered,
    /// bits 8.. its level; zero when no source is routed to it.
    line: u32,
    pending: [u32; SOURCE_WORDS],
    /// The sources routed to the vector in `mcause` on this hart.
    routed: [u32; SOURCE_WORDS],
}

oer_espressif_staged_runtime::retained_static! {
    /// The fatal record. The fatal path takes it without waiting: while the
    /// other hart records its own failure, this one's is skipped.
    static SLOT: Record;
}

/// Record a panic's machine state under `stop`; the platform resets the
/// chip next.
#[cfg(feature = "panic-hook")]
pub(crate) fn record_panic(claim: &Claim<'_>, stage: u32, stop: Result<&StopTheWorld<'_>, StopError>) {
    record(claim, PANIC, 0, stage, stop);
}

/// A boot check that halts the image.
#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum Failure {
    Layout = 1,
    TaskAllocation = 2,
    EntropySource = 3,
    RootTaken = 4,
    InterruptTable = 5,
    Console = 6,
    InterruptHart = 7,
    InterruptStack = 8,
    InterruptVectors = 9,
    InterruptControlBits = 10,
    InterruptCount = 11,
    InterruptLevelBits = 12,
    InterruptStackGuard = 13,
    RootInitialization = 14,
    NetworkContextAccessRefused = 15,
}

impl Failure {
    const ALL: [Self; 15] = [
        Self::Layout,
        Self::TaskAllocation,
        Self::EntropySource,
        Self::RootTaken,
        Self::InterruptTable,
        Self::Console,
        Self::InterruptHart,
        Self::InterruptStack,
        Self::InterruptVectors,
        Self::InterruptControlBits,
        Self::InterruptCount,
        Self::InterruptLevelBits,
        Self::InterruptStackGuard,
        Self::RootInitialization,
        Self::NetworkContextAccessRefused,
    ];

    pub(crate) fn from_boot(error: oer_espressif_staged_runtime::BootError) -> Self {
        use oer_espressif_interrupt_table::Error;
        use oer_espressif_staged_runtime::BootError;
        match error {
            BootError::WrongHart { .. } => Self::InterruptHart,
            BootError::NoInterruptStack { .. } => Self::InterruptStack,
            BootError::InvalidStackGuard { .. } => Self::InterruptStackGuard,
            BootError::ForeignVectors => Self::InterruptVectors,
            BootError::InterruptController(Error::WrongControlBits { .. }) => {
                Self::InterruptControlBits
            }
            BootError::InterruptController(Error::WrongInterruptCount { .. }) => {
                Self::InterruptCount
            }
            BootError::InterruptController(Error::WrongLevelBits { .. }) => {
                Self::InterruptLevelBits
            }
            BootError::InterruptController(_) => Self::InterruptTable,
        }
    }

    pub(crate) const fn name(self) -> oer_diagnostics::Name<32> {
        match self {
            Self::Layout => oer_diagnostics::name!("layout"; 32),
            Self::TaskAllocation => oer_diagnostics::name!("task-allocation"; 32),
            Self::EntropySource => oer_diagnostics::name!("entropy-source"; 32),
            Self::RootTaken => oer_diagnostics::name!("root-taken"; 32),
            Self::RootInitialization => oer_diagnostics::name!("root-initialization"; 32),
            Self::NetworkContextAccessRefused => {
                oer_diagnostics::name!("network-context-access-refused"; 32)
            }
            Self::InterruptTable => oer_diagnostics::name!("interrupt-table"; 32),
            Self::Console => oer_diagnostics::name!("console"; 32),
            Self::InterruptHart => oer_diagnostics::name!("interrupt-hart"; 32),
            Self::InterruptStack => oer_diagnostics::name!("interrupt-stack"; 32),
            Self::InterruptStackGuard => oer_diagnostics::name!("interrupt-stack-guard"; 32),
            Self::InterruptVectors => oer_diagnostics::name!("interrupt-vectors"; 32),
            Self::InterruptControlBits => oer_diagnostics::name!("interrupt-control-bits"; 32),
            Self::InterruptCount => oer_diagnostics::name!("interrupt-count"; 32),
            Self::InterruptLevelBits => oer_diagnostics::name!("interrupt-level-bits"; 32),
        }
    }

    fn from_word(word: u32) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|failure| *failure as u32 == word)
    }
}

/// Record a failed boot check for the next boot, whose note the console's
/// driver kept off the block. Never waits: while the other hart records its
/// own failure, this one's is skipped.
pub(crate) fn record_failure(claim: &Claim<'_>, failure: Failure) {
    let mut payload: Payload = bytemuck::Zeroable::zeroed();
    payload.kind = FAILURE;
    payload.hart = TrapRegisters::read().hart as u32;
    payload.stage = failure as u32;
    commit(claim, payload);

}

/// Record an exception's machine state and its `ra` under `stop`; the
/// runtime's fatal reset follows.
pub(crate) fn record_exception(claim: &Claim<'_>, ra: u32, stop: Result<&StopTheWorld<'_>, StopError>) {
    record(claim, EXCEPTION, ra, 0, stop);
}

fn record(claim: &Claim<'_>, kind: u32, ra: u32, stage: u32, stop: Result<&StopTheWorld<'_>, StopError>) {
    let TrapRegisters {
        hart,
        mcause,
        mepc,
        mtval,
        mscratch,
        mtvec,
        mtvt,
    } = TrapRegisters::read();
    let vector = mcause & 0x0fff;
    let mtvt_slot = trap::current_hart_vector_slot(vector).unwrap_or(0);
    let mut pending = [0; SOURCE_WORDS];
    // An invalid raw ID is retained by the runtime's fatal path; it grants
    // no other hart's bank for this diagnostic sample.
    let checked_hart = oer_esp32s31_hal::platform::Hart::new(hart as u32);
    for source in 0..SOURCE_WORDS * 32 {
        // At most 192 sources: the count fits a source number.
        if checked_hart.is_ok_and(|hart| interrupt_controller::is_pending(hart, source as u16)) {
            set(&mut pending, source);
        }
    }
    let mut routed = [0; SOURCE_WORDS];
    let mut line = 0;
    if let Ok(stop) = stop {
        let _observed = interrupt_controller::fatal_view(stop, |view| {
            for source in 0..SOURCE_WORDS * 32 {
                if view.routed(source as u16).map(usize::from) == Some(vector) {
                    set(&mut routed, source);
                }
            }
            if routed != [0; SOURCE_WORDS]
                && let Some(observed) = u8::try_from(vector)
                    .ok()
                    .and_then(|vector| view.line(vector))
            {
                line = line_word(observed);
            }
        });
    }
    // The other hart may hold any lock while this one fails, so the slot
    // is taken without waiting; when that hart is recording its own
    // failure, this one's is skipped.
    let payload = Payload {
        kind,
        hart: hart as u32,
        mcause: mcause as u32,
        mepc: mepc as u32,
        mtval: mtval as u32,
        ra,
        mscratch: mscratch as u32,
        mtvec: mtvec as u32,
        mtvt: mtvt as u32,
        mtvt_slot,
        stage,
        line,
        pending,
        routed,
    };
    commit(claim, payload);
}

/// The record's word of an observed line: bit 0 pending, 1 enabled, 2
/// edge-triggered, bits 8.. its level byte.
fn line_word(observed: LineObservation) -> u32 {
    let LineObservation { pending, state } = observed;
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
#[inline(always)]
fn set(words: &mut [u32; SOURCE_WORDS], source: usize) {
    if let Some(word) = words.get_mut(source / 32) {
        *word |= 1 << (source % 32);
    }
}

/// Print the previous record early at boot; acknowledge only after every
/// note was successfully written.
pub(crate) fn report_previous() {
    if let Some(mut slot) = retained_fault::peek(&SLOT) {
        let _reported = slot.report(|view| {
            let payload: Payload = view.decode(Domain::Hil).map_err(|_| ())?;
            report_payload(payload, |note| crate::usb::early(note).then_some(()).ok_or(()))
        });
    }
}
fn commit(claim: &Claim<'_>, payload: Payload) {
    let class = match payload.kind {
        EXCEPTION => Class::CpuException,
        FAILURE => Class::ConfigurationBoot,
        _ => Class::RustPanic,
    };
    let [p0,p1,p2,p3,p4,p5] = payload.pending;
    let [r0,r1,r2,r3,r4,r5] = payload.routed;
    let words = [payload.kind, payload.hart, payload.mcause, payload.mepc, payload.mtval,
        payload.ra, payload.mscratch, payload.mtvec, payload.mtvt, payload.mtvt_slot,
        payload.stage, payload.line, p0,p1,p2,p3,p4,p5,r0,r1,r2,r3,r4,r5];
    if let Ok(record) = Record::encode(class, Domain::Hil, payload.kind,
        payload.hart, payload.stage, &words) {
        let _committed = claim.commit(&SLOT, record);
    }
}
fn report_payload(payload: Payload, mut emit: impl FnMut(&Note<'_>) -> Result<(), ()>) -> Result<(), ()> {
    let [p0,p1,p2,p3,p4,p5] = payload.pending;
    let [r0,r1,r2,r3,r4,r5] = payload.routed;
    let Payload { kind, hart, mcause, mepc, mtval, ra, mscratch, stage,
        line, mtvec, mtvt, mtvt_slot, .. } = payload;
    if kind == FAILURE {
        let failure = Failure::from_word(stage).ok_or(())?;
        return emit(&Note::fatal_fail(hart, failure.name()));
    }
    if kind == EXCEPTION {
        emit(&Note::fatal_exception(hart, mcause, mepc, mtval, ra, mscratch))?;
    } else {
        emit(&Note::fatal_panic(hart, stage, mcause, mepc, mtval))?;
    }
    emit(&Note::fatal_irq_vector(mcause & 0x0fff, line, mtvec, mtvt, mtvt_slot))?;
    emit(&Note::fatal_irq_pending(p0,p1,p2,p3,p4,p5))?;
    emit(&Note::fatal_irq_routed(r0,r1,r2,r3,r4,r5))
}
