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
    _unused: u32,
}

/// The bytes of a [`TrapFrame`], a multiple of the stack's 16-byte alignment.
pub const TRAP_FRAME_BYTES: usize = core::mem::size_of::<TrapFrame>();

const _: () = assert!(TRAP_FRAME_BYTES.is_multiple_of(16));

/// Vectors of a hart's CLIC vector table (MTVT) that hold an entry.
#[cfg(feature = "esp32s31")]
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
