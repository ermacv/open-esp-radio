//! The hart's trap state as a fatal path records it: the trap CSRs, the
//! vector table entry the hart last dispatched through and the stack
//! watchpoint. Every read is inlined, so a caller placed in SRAM stays there.

use core::arch::asm;

pub use esp_hal::trapframe::TrapFrame;

/// Vectors of the CLIC vector table (MTVT).
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
                "csrr {mtvt}, 0x307",
                hart = out(reg) hart,
                mcause = out(reg) mcause,
                mepc = out(reg) mepc,
                mtval = out(reg) mtval,
                mscratch = out(reg) mscratch,
                mtvec = out(reg) mtvec,
                mtvt = out(reg) mtvt,
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
#[inline(always)]
pub fn current_hart_vector_slot(vector: usize) -> Option<u32> {
    if vector >= VECTORS {
        return None;
    }
    let table: *const u32;
    // SAFETY: a CSR read only.
    unsafe { asm!("csrr {table}, 0x307", table = out(reg) table, options(nomem, nostack)) };
    // SAFETY: MTVT addresses the hart's word-aligned table of `VECTORS`
    // entries in SRAM, which the runtime installed; reading one entry calls
    // nothing.
    Some(unsafe { table.add(vector).read_volatile() })
}

/// The current trigger's control (`tdata1`) and address (`tdata2`): after
/// the handoff, the task stack's watchpoint.
#[inline(always)]
pub fn stack_watchpoint() -> (usize, usize) {
    let (control, address);
    // SAFETY: CSR reads only.
    unsafe {
        asm!(
            "csrr {control}, 0x7a1",
            "csrr {address}, 0x7a2",
            control = out(reg) control,
            address = out(reg) address,
            options(nomem, nostack),
        )
    };
    (control, address)
}

/// The current stack pointer.
#[inline(always)]
pub fn stack_pointer() -> usize {
    let value;
    // SAFETY: a register read only.
    unsafe { asm!("mv {value}, sp", value = out(reg) value, options(nomem, nostack)) };
    value
}
