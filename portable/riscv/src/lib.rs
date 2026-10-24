#![no_std]
#![forbid(unsafe_code)]

//! Architectural RISC-V constants that firmware and analysis share: CSR
//! numbers and the fields of the CSRs this repository programs or decodes.
//!
//! The crate holds definitions only, with no chip parameter (a chip's
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

/// The Core-Local Interrupt Controller's CSRs.
pub mod clic {
    /// `mtvt`: base of the table of hardware-vectored handler addresses.
    ///
    /// SOURCE: RISC-V Core-Local Interrupt Controller (CLIC) specification,
    /// "CLIC CSRs", `mtvt` (CSR 0x307).
    pub const MTVT: u16 = 0x307;

    /// `mnxti`: takes the next pending interrupt above the current level (a
    /// write form lowers `mintstatus.mil` to that interrupt's level).
    ///
    /// SOURCE: RISC-V CLIC specification, "CLIC CSRs", `mnxti` (CSR 0x345).
    pub const MNXTI: u16 = 0x345;

    /// `mintstatus`: read-only machine interrupt status (`mil`).
    ///
    /// SOURCE: RISC-V CLIC specification, "CLIC CSRs", `mintstatus`
    /// (CSR 0xFB1).
    pub const MINTSTATUS: u16 = 0xfb1;

    /// `mintthresh`: machine interrupt-level threshold.
    ///
    /// SOURCE: RISC-V CLIC specification, "CLIC CSRs", `mintthresh`
    /// (CSR 0x347).
    pub const MINTTHRESH: u16 = 0x347;
}

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
    }
}
