//! The hart's trap state as a fatal path records it: the trap frame the
//! runtime's entries save, the trap CSRs, the vector table entry the hart
//! last dispatched through. Every read is inlined,
//! so a caller placed in SRAM stays there.

/// The frame every trap entry saves on the hart's SRAM interrupt stack
/// (`vectors.s`): the interrupted caller registers and the trap CSRs, as the
/// exception handler receives it.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct TrapFrame {
    pub ra: u32,
    pub t0: u32,
    pub t1: u32,
    pub t2: u32,
    pub t3: u32,
    pub t4: u32,
    pub t5: u32,
    pub t6: u32,
    pub a0: u32,
    pub a1: u32,
    pub a2: u32,
    pub a3: u32,
    pub a4: u32,
    pub a5: u32,
    pub a6: u32,
    pub a7: u32,
    /// Non-zero for the outermost trap, which preempted a task.
    pub outermost: u32,
    /// `mstatus` at the trap.
    pub mstatus: u32,
    /// The interrupted stack pointer: the task's for the outermost trap,
    /// the interrupted handler's for a nested one.
    pub sp: u32,
    pub mepc: u32,
    pub mcause: u32,
    /// The CLIC threshold (`mintthresh`) at the trap.
    pub mintthresh: u32,
    /// `tcontrol` as the trap left it: its MPTE is the interrupted
    /// context's MTE.
    pub tcontrol: u32,
    /// Entry-time trap value, captured before the Rust handler or platform work.
    pub mtval: u32,
}

/// The bytes of a [`TrapFrame`], a multiple of the stack's 16-byte alignment.
pub const TRAP_FRAME_BYTES: usize = core::mem::size_of::<TrapFrame>();

const _: () = {
    if TRAP_FRAME_BYTES != 96 {
        panic!("trap entry uses the existing 96-byte aligned frame");
    }
};

/// Maximum simultaneous exception frames with intact SRAM and trap/mask code:
/// capture/hook, a reentry requesting reset, and a fault in that one reset
/// attempt. Further attempts return directly to the architectural halt.
/// The stack analyser reads the assembly-exported value of this constant.
pub const MAX_EXCEPTION_FRAMES: u32 = 3;

/// Vectors of a hart's CLIC vector table (MTVT) that hold an entry.
#[cfg(chip_vectors)]
pub const VECTORS: usize = crate::interrupts::LINES.count as usize;

/// The current hart's trap CSRs.
#[derive(Clone, Copy, Debug)]
pub struct TrapRegisters {
    pub hart: usize,
    pub mcause: usize,
    pub mepc: usize,
    pub mtval: usize,
    pub mscratch: usize,
    pub mtvec: usize,
    pub mtvt: usize,
}

#[cfg(target_arch = "riscv32")]
impl TrapRegisters {
    /// Read the current hart's trap CSRs.
    #[inline(always)]
    pub fn read() -> Self {
        let hart = oer_riscv::hart_id();
        let mcause = oer_riscv::read_mcause();
        let mepc = oer_riscv::read_mepc();
        let mtval = oer_riscv::read_mtval();
        let mscratch = oer_riscv::read_mscratch();
        let mtvec = oer_riscv::read_mtvec();
        let mtvt = oer_riscv::read_mtvt();
        Self {
            hart,
            mcause,
            mepc,
            mtval,
            mscratch,
            mtvec,
            mtvt,
        }
    }
}

/// The handler the current hart's active vector table (MTVT) holds for
/// `vector`; `None` beyond the table.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
pub fn current_hart_vector_slot(vector: usize) -> Option<u32> {
    if vector >= VECTORS {
        return None;
    }
    let table = oer_riscv::read_mtvt() as *const u32;
    // SAFETY: MTVT addresses the hart's word-aligned table of `VECTORS`
    // entries in SRAM, which the runtime installed; reading one entry calls
    // nothing.
    Some(unsafe { table.add(vector).read_volatile() })
}

use crate::retained_fault::{Claim, Reentered, Terminal};
use oer_diagnostics::fault::{Class, Domain, Record, View};

/// Numeric reason in the CPU-exception namespace.
const CPU_EXCEPTION: u32 = 1;
/// The exception record's words: the frame, `mscratch` and the terminal.
const EXCEPTION_WORDS: usize = 26;

fn encode_exception(
    frame: &TrapFrame,
    hart: u32,
    mscratch: u32,
    terminal: Option<Terminal>,
) -> Result<Record, oer_diagnostics::fault::Error> {
    let words = [
        frame.ra,
        frame.t0,
        frame.t1,
        frame.t2,
        frame.t3,
        frame.t4,
        frame.t5,
        frame.t6,
        frame.a0,
        frame.a1,
        frame.a2,
        frame.a3,
        frame.a4,
        frame.a5,
        frame.a6,
        frame.a7,
        frame.outermost,
        frame.mstatus,
        frame.sp,
        frame.mepc,
        frame.mcause,
        frame.mintthresh,
        frame.tcontrol,
        frame.mtval,
        mscratch,
        Terminal::word(terminal),
    ];
    Record::encode(
        Class::CpuException,
        Domain::CpuException,
        CPU_EXCEPTION,
        hart,
        frame.outermost,
        &words,
    )
}

// Reentry never recurses into the recorder or overwrites the first terminal.
fn capture_first<'a>(
    claim: Result<Claim<'a>, Reentered>,
    record: impl FnOnce(&Claim<'_>),
) -> Result<Claim<'a>, Reentered> {
    let claim = claim?;
    record(&claim);
    Ok(claim)
}

/// Common terminal exception path. The entry has captured the CSRs and disabled
/// the FPU. Commit the minimum before stop/MMIO/supplemental diagnostics. A
/// repeated fault skips those operations and requests reset directly, retaining
/// the first committed frame. A fault before frame/commit completion remains
/// subject to hardware protection and the preserved watchdog (best effort).
#[cfg(all(target_arch = "riscv32", chip_vectors))]
pub fn handle<P: oer_espressif_platform_port::Platform>(
    frame: &TrapFrame,
    hook: impl for<'a, 'b> FnOnce(&Claim<'_>, &TrapFrame, Result<&'b P::Stop<'a>, P::StopError>),
) -> ! {
    P::fatal(|fatal| {
        let captured = capture_first(crate::retained_fault::claim(), |claim| {
            if let Ok(record) = encode_exception(
                frame,
                oer_riscv::hart_id() as u32,
                oer_riscv::read_mscratch() as u32,
                claim.terminal(),
            ) {
                let _committed = claim.commit(&crate::panic::SLOT, record);
            }
        });
        let Ok(claim) = captured else {
            crate::fatal::reset_platform_once::<P>(fatal);
            return;
        };
        crate::fatal::reset_claimed::<P>(&claim, |claim, stop| hook(claim, frame, stop))
    })
}

/// The previous boot's CPU exception as the trap left it: the registers the
/// runtime retained before it stopped the other hart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviousException {
    /// The hart that trapped.
    pub hart: u32,
    pub mcause: u32,
    pub mepc: u32,
    pub mtval: u32,
    pub ra: u32,
    pub sp: u32,
    pub mscratch: u32,
    /// The terminal the trap claimed; a record another slot holds belongs
    /// to this trap only when it names the same one.
    pub terminal: Option<Terminal>,
}

/// The previous boot's CPU exception, without printing or clearing its
/// record: read before adoption reports it, so that the boot can serve it.
#[cfg(target_arch = "riscv32")]
pub fn peek_previous_exception() -> Option<PreviousException> {
    let slot = crate::retained_fault::peek(&crate::panic::SLOT)?;
    previous_exception(&slot.peek().ok()?)
}

/// The exception a retained record holds; `None` for any other record.
fn previous_exception(view: &View<'_>) -> Option<PreviousException> {
    if view.reason != CPU_EXCEPTION || view.class != Class::CpuException {
        return None;
    }
    let [ra, .., sp, mepc, mcause, _, _, mtval, mscratch, terminal] = view
        .decode::<[u32; EXCEPTION_WORDS]>(Domain::CpuException)
        .ok()?;
    Some(PreviousException {
        hart: view.hart,
        mcause,
        mepc,
        mtval,
        ra,
        sp,
        mscratch,
        terminal: Terminal::from_word(terminal),
    })
}

pub(crate) fn report_exception(
    view: View<'_>,
    mut emit: impl FnMut(&[u8]) -> Result<(), ()>,
) -> Result<(), ()> {
    if view.reason != CPU_EXCEPTION || view.class != Class::CpuException {
        return Err(());
    }
    let [ra, .., mepc, mcause, _, _, mtval, mscratch, terminal] = view
        .decode::<[u32; EXCEPTION_WORDS]>(Domain::CpuException)
        .map_err(|_| ())?;
    oer_diagnostics::emit_note(
        &oer_diagnostics::Note::fatal_exception(view.hart, mcause, mepc, mtval, ra, mscratch),
        &mut emit,
    )
    .map_err(|_| ())?;
    // The number the image's own notes of this trap repeat.
    match Terminal::from_word(terminal) {
        Some(terminal) => oer_diagnostics::emit_note(
            &oer_diagnostics::Note::fatal_terminal(view.hart, terminal.get()),
            emit,
        )
        .map_err(|_| ()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> TrapFrame {
        TrapFrame {
            ra: 1,
            t0: 2,
            t1: 3,
            t2: 4,
            t3: 5,
            t4: 6,
            t5: 7,
            t6: 8,
            a0: 9,
            a1: 10,
            a2: 11,
            a3: 12,
            a4: 13,
            a5: 14,
            a6: 15,
            a7: 16,
            outermost: 1,
            mstatus: 0x1800,
            sp: 0x2ff12340,
            mepc: 0x4fe00004,
            mcause: 5,
            mintthresh: 0,
            tcontrol: 0,
            mtval: 0xdeadbeef,
        }
    }
    #[test]
    fn entry_snapshot_round_trips_and_refused_output_keeps_it() {
        let frame = frame();
        let mut record = encode_exception(&frame, 1, 0x4ff12340, Terminal::from_word(7)).unwrap();
        let view = record.peek().unwrap();
        assert_eq!(
            (view.class, view.hart, view.context),
            (Class::CpuException, 1, 1)
        );
        let words = view
            .decode::<[u32; EXCEPTION_WORDS]>(Domain::CpuException)
            .unwrap();
        assert_eq!(
            (words[19], words[20], words[23], words[25]),
            (frame.mepc, frame.mcause, frame.mtval, 7)
        );
        assert!(
            record
                .report(|view| report_exception(view, |_| Err(())))
                .is_err()
        );
        let mut bytes = Vec::new();
        record
            .report(|view| {
                report_exception(view, |part| {
                    bytes.extend_from_slice(part);
                    Ok(())
                })
            })
            .unwrap();
        // `fatal_exception`, then its terminal's note.
        assert_eq!(frames(&bytes), 2);
        assert!(record.peek().is_err());
    }
    #[test]
    fn a_refused_terminal_note_keeps_the_record_and_an_unnumbered_trap_prints_none() {
        let mut record = encode_exception(&frame(), 1, 0x4ff12340, Terminal::from_word(7)).unwrap();
        let mut written = Vec::new();
        assert!(
            record
                .report(|view| report_exception(view, |part| {
                    // The exception's frame is whole; the terminal's is refused.
                    written.extend_from_slice(part);
                    if frames(&written) > 1 {
                        return Err(());
                    }
                    Ok(())
                }))
                .is_err()
        );
        assert!(record.peek().is_ok());
        let mut unnumbered = encode_exception(&frame(), 1, 0x4ff12340, None).unwrap();
        let mut bytes = Vec::new();
        unnumbered
            .report(|view| {
                report_exception(view, |part| {
                    bytes.extend_from_slice(part);
                    Ok(())
                })
            })
            .unwrap();
        assert_eq!(frames(&bytes), 1);
    }
    /// The console frames in `bytes`.
    fn frames(bytes: &[u8]) -> usize {
        let prefix = oer_diagnostics::console::PREFIX;
        bytes.windows(prefix.len()).filter(|w| *w == prefix).count()
    }
    #[test]
    fn the_previous_exception_is_read_without_clearing_and_only_from_its_record() {
        let frame = frame();
        let record = encode_exception(&frame, 1, 0x4ff12340, Terminal::from_word(9)).unwrap();
        assert_eq!(
            previous_exception(&record.peek().unwrap()),
            Some(PreviousException {
                hart: 1,
                mcause: frame.mcause,
                mepc: frame.mepc,
                mtval: frame.mtval,
                ra: frame.ra,
                sp: frame.sp,
                mscratch: 0x4ff12340,
                terminal: Terminal::from_word(9),
            })
        );
        // Reading it leaves the record for the boot's report.
        assert!(record.peek().is_ok());
        let panic = Record::new(Class::RustPanic, Domain::Panic, 1, 0, 0, &[7]).unwrap();
        assert_eq!(previous_exception(&panic.peek().unwrap()), None);
    }
    #[test]
    fn repeated_exception_skips_capture_and_keeps_the_first_minimum() {
        let election = crate::retained_fault::Election::new();
        crate::retained_static! {
            static NUMBERS: u32;
        }
        let mut minimum = Record::EMPTY;
        let mut order = Vec::new();
        let _first = capture_first(election.claim(&NUMBERS), |claim| {
            minimum
                .commit(encode_exception(&frame(), 0, 0x4ff12340, claim.terminal()).unwrap())
                .unwrap();
            order.push("commit");
        })
        .unwrap();
        // Model a fault during stop/the supplemental callback.
        order.push("stop");
        let first = minimum;
        assert!(capture_first(election.claim(&NUMBERS), |_| panic!("recursive recorder")).is_err());
        order.push("reset");
        assert_eq!(minimum, first);
        assert_eq!(order, ["commit", "stop", "reset"]);
    }
}
