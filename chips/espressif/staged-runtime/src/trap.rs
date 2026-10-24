//! The hart's trap state as a fatal path records it: the trap frame the
//! runtime's entries save, the trap CSRs, the vector table entry the hart
//! last dispatched through and the stack watchpoint. Every read is inlined,
//! so a caller placed in SRAM stays there.

#[cfg(target_arch = "riscv32")]
use core::arch::asm;

#[cfg(target_arch = "riscv32")]
use oer_riscv::{clic, trigger};

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
    _unused: u32,
}

/// The bytes of a [`TrapFrame`], a multiple of the stack's 16-byte alignment.
pub const TRAP_FRAME_BYTES: usize = 96;

// `vectors.s` stores the frame by these offsets.
const _: () = {
    use core::mem::{offset_of, size_of};
    assert!(size_of::<TrapFrame>() == TRAP_FRAME_BYTES);
    assert!(offset_of!(TrapFrame, a0) == 32 && offset_of!(TrapFrame, a7) == 60);
    assert!(offset_of!(TrapFrame, outermost) == 64);
    assert!(offset_of!(TrapFrame, mstatus) == 68);
    assert!(offset_of!(TrapFrame, sp) == 72);
    assert!(offset_of!(TrapFrame, mepc) == 76);
    assert!(offset_of!(TrapFrame, mcause) == 80);
    assert!(offset_of!(TrapFrame, mintthresh) == 84);
    assert!(offset_of!(TrapFrame, tcontrol) == 88);
};

/// Vectors of a hart's CLIC vector table (MTVT) that hold an entry.
pub const VECTORS: usize = 48;

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
        let (hart, mcause, mepc, mtval, mscratch, mtvec, mtvt);
        // SAFETY: CSR reads only.
        unsafe {
            asm!(
                "csrr {hart}, mhartid",
                "csrr {mcause}, mcause",
                "csrr {mepc}, mepc",
                "csrr {mtval}, mtval",
                "csrr {mscratch}, mscratch",
                "csrr {mtvec}, mtvec",
                "csrr {mtvt}, {mtvt_csr}",
                hart = out(reg) hart,
                mcause = out(reg) mcause,
                mepc = out(reg) mepc,
                mtval = out(reg) mtval,
                mscratch = out(reg) mscratch,
                mtvec = out(reg) mtvec,
                mtvt = out(reg) mtvt,
                mtvt_csr = const clic::MTVT,
                options(nomem, nostack),
            )
        };
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
    let table: *const u32;
    // SAFETY: a CSR read only.
    unsafe {
        asm!("csrr {table}, {mtvt}", table = out(reg) table, mtvt = const clic::MTVT, options(nomem, nostack))
    };
    // SAFETY: MTVT addresses the hart's word-aligned table of `VECTORS`
    // entries in SRAM, which the runtime installed; reading one entry calls
    // nothing.
    Some(unsafe { table.add(vector).read_volatile() })
}

/// The current trigger's control (`tdata1`) and address (`tdata2`): after
/// the handoff, the task stack's watchpoint.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
pub fn stack_watchpoint() -> (usize, usize) {
    let (control, address);
    // SAFETY: CSR reads only.
    unsafe {
        asm!(
            "csrr {control}, {tdata1}",
            "csrr {address}, {tdata2}",
            tdata1 = const trigger::TDATA1,
            tdata2 = const trigger::TDATA2,
            control = out(reg) control,
            address = out(reg) address,
            options(nomem, nostack),
        )
    };
    (control, address)
}

/// The current stack pointer.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
pub fn stack_pointer() -> usize {
    let value;
    // SAFETY: a register read only.
    unsafe { asm!("mv {value}, sp", value = out(reg) value, options(nomem, nostack)) };
    value
}
