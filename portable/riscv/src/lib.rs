#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! Architectural RISC-V instructions and constants: CSR access, local interrupt
//! admission, waits, barriers, and the definitions firmware and analysis share.
//!
//! The crate holds architectural definitions and instructions, with no chip parameter (a chip's
//! interrupt-line count or trigger budget lives with the chip). CSR numbers
//! are `u16` for decoders and `const` operands of `asm!` and `global_asm!`;
//! register values are `usize` (XLEN).

/// Machine-level CSRs of the privileged architecture.
pub mod machine {
    /// `mscratch`.
    ///
    /// SOURCE: RISC-V Privileged Architecture, "Machine-Mode Scratch Register
    /// (mscratch)" and the CSR address map (Table "Currently allocated RISC-V
    /// machine-level CSR addresses").
    pub const MSCRATCH: u16 = 0x340;

    /// `mstatus`.
    ///
    /// SOURCE: RISC-V Privileged Architecture, CSR address map (`mstatus`
    /// 0x300).
    pub const MSTATUS: u16 = 0x300;

    /// `mtvec`: the trap vector base and mode (mode 3 selects the CLIC).
    ///
    /// SOURCE: RISC-V Privileged Architecture, CSR address map (`mtvec`
    /// 0x305); RISC-V CLIC specification, "mtvec" (mode `0b11`).
    pub const MTVEC: u16 = 0x305;

    /// `mepc`: the pc a trap return resumes.
    ///
    /// SOURCE: RISC-V Privileged Architecture, CSR address map (`mepc`
    /// 0x341).
    pub const MEPC: u16 = 0x341;

    /// `mcause`: the trap's cause; under the CLIC it also holds the
    /// interrupted level (`mpil`), which a trap return restores.
    ///
    /// SOURCE: RISC-V Privileged Architecture, CSR address map (`mcause`
    /// 0x342); RISC-V CLIC specification, "mcause" (`mpil`, `mpie`, `mpp`).
    pub const MCAUSE: u16 = 0x342;

    /// `mtvec` mode 3: CLIC mode.
    ///
    /// SOURCE: RISC-V CLIC specification, "mtvec" (mode `0b11`).
    pub const MTVEC_CLIC_MODE: usize = 0b11;

    /// The floating-point CSRs, numbered consecutively.
    ///
    /// SOURCE: RISC-V Unprivileged ISA, "Floating-Point Control and Status
    /// Register" (`fflags` 0x001, `frm` 0x002, `fcsr` 0x003).
    pub mod float {
        /// `fflags`: accrued exception flags.
        pub const FFLAGS: u16 = 0x001;
        /// `fcsr`: `frm` and `fflags` together.
        pub const FCSR: u16 = 0x003;
    }

    /// `mstatus` bits.
    pub mod status {
        /// `mstatus.MIE`, bit 3: machine-mode global interrupt enable.
        ///
        /// SOURCE: RISC-V Privileged Architecture, "Machine Status Registers
        /// (mstatus and mstatush)".
        pub const MIE: usize = 1 << 3;

        /// `mstatus.FS`, bits 14:13: the floating-point unit's state (off,
        /// initial, clean, dirty). All bits set is dirty; the mask selects the
        /// field.
        ///
        /// SOURCE: RISC-V Privileged Architecture, "Extension Context
        /// Status in mstatus Register" (`FS` field).
        pub const FS_MASK: usize = 0b11 << 13;
    }
}

/// Core-Local Interrupt Controller CSRs and typed line encodings.
pub mod clic;

/// The debug-trigger module's CSRs and the `mcontrol` trigger encoding.
pub mod trigger {
    /// `tselect`: selects the trigger the other trigger CSRs address.
    ///
    /// SOURCE: RISC-V Debug Specification (Sdtrig), "Trigger Select Register
    /// (tselect, at 0x7a0)".
    pub const TSELECT: u16 = 0x7a0;

    /// `tdata1`: the selected trigger's type and control.
    ///
    /// SOURCE: RISC-V Debug Specification (Sdtrig), "Trigger Data 1 (tdata1,
    /// at 0x7a1)".
    pub const TDATA1: u16 = 0x7a1;

    /// `tdata2`: the selected trigger's first data word (`mcontrol`: the
    /// address).
    ///
    /// SOURCE: RISC-V Debug Specification (Sdtrig), "Trigger Data 2 (tdata2,
    /// at 0x7a2)".
    pub const TDATA2: u16 = 0x7a2;

    /// `tdata3`: the selected trigger's third data word.
    ///
    /// SOURCE: RISC-V Debug Specification (Sdtrig), "Trigger Data 3 (tdata3,
    /// at 0x7a3)".
    pub const TDATA3: u16 = 0x7a3;

    /// `tcontrol`: machine-mode trigger enable state.
    ///
    /// SOURCE: RISC-V Debug Specification (Sdtrig), "Trigger Control
    /// (tcontrol, at 0x7a5)".
    pub const TCONTROL: u16 = 0x7a5;

    /// `tcontrol.mte`, bit 3: triggers fire in machine mode.
    ///
    /// SOURCE: RISC-V Debug Specification (Sdtrig), "Trigger Control
    /// (tcontrol, at 0x7a5)", field `mte`.
    pub const TCONTROL_MTE: usize = 1 << 3;

    /// `tcontrol.mpte`, bit 7: `mte` before the last trap into machine
    /// mode, which a trap sets from `mte` and `mret` copies back.
    ///
    /// SOURCE: RISC-V Debug Specification (Sdtrig), "Trigger Control
    /// (tcontrol, at 0x7a5)", field `mpte`.
    pub const TCONTROL_MPTE: usize = 1 << 7;

    /// Fields of `tdata1` for an address/data match trigger (`mcontrol`).
    pub mod mcontrol {
        /// `mcontrol.m`, bit 6: the trigger fires in machine mode.
        ///
        /// SOURCE: RISC-V Debug Specification (Sdtrig), "Match Control
        /// (mcontrol, at 0x7a1)", field `m`.
        pub const M: usize = 1 << 6;

        /// `mcontrol.store`, bit 1: the trigger fires on a store address.
        ///
        /// SOURCE: RISC-V Debug Specification (Sdtrig), "Match Control
        /// (mcontrol, at 0x7a1)", field `store`.
        pub const STORE: usize = 1 << 1;

        /// `mcontrol.match`, bits 10:7, value 1: `tdata2` is a naturally
        /// aligned power-of-two region (NAPOT) with the size in its trailing
        /// ones.
        ///
        /// SOURCE: RISC-V Debug Specification (Sdtrig), "Match Control
        /// (mcontrol, at 0x7a1)", field `match`, value 1.
        pub const MATCH_NAPOT: usize = 1 << 7;

        /// `tdata1.dmode`, bit XLEN-5 (27 on RV32): only debug mode may
        /// write the trigger; machine-mode writes are ignored. A connected
        /// debugger that owns the trigger sets it.
        ///
        /// SOURCE: RISC-V Debug Specification (Sdtrig), "Trigger Data 1
        /// (tdata1, at 0x7a1)", field `dmode`.
        pub const DMODE: usize = 1 << (usize::BITS - 5);
    }
}

#[cfg(target_arch = "riscv32")]
#[allow(unsafe_code, reason = "architectural CSR and wait instructions")]
mod instructions;
#[cfg(target_arch = "riscv32")]
pub use instructions::*;

/// Order every device-memory access before this call against every one
/// after it: the fence a driver places at a hardware ownership boundary.
#[inline]
pub fn fence() {
    #[cfg(target_arch = "riscv32")]
    #[allow(unsafe_code, reason = "an ordering instruction")]
    // SAFETY: the instruction only orders memory and device accesses; it
    // reads and writes no memory and changes no register Rust relies on.
    unsafe {
        core::arch::asm!("fence iorw, iorw", options(nostack, preserves_flags))
    }
    #[cfg(not(target_arch = "riscv32"))]
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}
