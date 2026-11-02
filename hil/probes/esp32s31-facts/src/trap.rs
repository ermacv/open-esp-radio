//! The facts image's own trap ABI: no production runtime dependency.
//! Entries retain all integer caller state, the interrupted SP, and trap CSRs.

#[repr(C)]
struct TrapFrame {
    ra: u32,
    t0: u32,
    t1: u32,
    t2: u32,
    t3: u32,
    t4: u32,
    t5: u32,
    t6: u32,
    a0: u32,
    a1: u32,
    a2: u32,
    a3: u32,
    a4: u32,
    a5: u32,
    a6: u32,
    a7: u32,
    outermost: u32,
    mstatus: u32,
    sp: u32,
    mepc: u32,
    mcause: u32,
    mintthresh: u32,
    tcontrol: u32,
    _unused: u32,
}

const FRAME_BYTES: usize = core::mem::size_of::<TrapFrame>();
const _: () = assert!(FRAME_BYTES.is_multiple_of(16));

#[cfg(target_arch = "riscv32")]
// SAFETY: this image alone defines its trap symbols and generated vector table.
// Both exclusive stacks live in SRAM, with IRQ below task; entry swaps stacks
// before its first memory access and exit restores the interrupted ABI state.
core::arch::global_asm!(
    include_str!(concat!(env!("OUT_DIR"), "/vectors.s")),
    include_str!("trap.s"),
    frame_bytes = const FRAME_BYTES,
    frame_ra = const core::mem::offset_of!(TrapFrame, ra),
    frame_t0 = const core::mem::offset_of!(TrapFrame, t0),
    frame_t1 = const core::mem::offset_of!(TrapFrame, t1),
    frame_t2 = const core::mem::offset_of!(TrapFrame, t2),
    frame_t3 = const core::mem::offset_of!(TrapFrame, t3),
    frame_t4 = const core::mem::offset_of!(TrapFrame, t4),
    frame_t5 = const core::mem::offset_of!(TrapFrame, t5),
    frame_t6 = const core::mem::offset_of!(TrapFrame, t6),
    frame_a0 = const core::mem::offset_of!(TrapFrame, a0),
    frame_a1 = const core::mem::offset_of!(TrapFrame, a1),
    frame_a2 = const core::mem::offset_of!(TrapFrame, a2),
    frame_a3 = const core::mem::offset_of!(TrapFrame, a3),
    frame_a4 = const core::mem::offset_of!(TrapFrame, a4),
    frame_a5 = const core::mem::offset_of!(TrapFrame, a5),
    frame_a6 = const core::mem::offset_of!(TrapFrame, a6),
    frame_a7 = const core::mem::offset_of!(TrapFrame, a7),
    frame_outermost = const core::mem::offset_of!(TrapFrame, outermost),
    frame_mstatus = const core::mem::offset_of!(TrapFrame, mstatus),
    frame_sp = const core::mem::offset_of!(TrapFrame, sp),
    frame_mepc = const core::mem::offset_of!(TrapFrame, mepc),
    frame_mcause = const core::mem::offset_of!(TrapFrame, mcause),
    frame_mintthresh = const core::mem::offset_of!(TrapFrame, mintthresh),
    frame_tcontrol = const core::mem::offset_of!(TrapFrame, tcontrol),
    fs = const oer_riscv::machine::status::FS_MASK,
    mie = const oer_riscv::machine::status::MIE,
    mintthresh = const oer_riscv::clic::MINTTHRESH,
    tcontrol = const oer_riscv::trigger::TCONTROL,
    mte = const oer_riscv::trigger::TCONTROL_MTE,
    mpte_shift = const (oer_riscv::trigger::TCONTROL_MPTE.trailing_zeros() - oer_riscv::trigger::TCONTROL_MTE.trailing_zeros()),
);
