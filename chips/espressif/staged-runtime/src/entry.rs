// Stage two does not return through the bootloader reset entry. It owns data,
// BSS, the SRAM interrupt closure and vector registers, and initializes each
// one before entering Rust or enabling interrupts. It runs on the facts-defined
// boot hart only; a secondary hart enters only through its generated entry.
core::arch::global_asm!(
    r#"
    .section .text._start, "ax", @progbits
    .balign 4
    .global _runtime_start
    .type _runtime_start, @function
_runtime_start:
    .option push
    .option norelax
    csrr t0, mhartid
    li t1, {boot_hart}
    bne t0, t1, 99f
    la gp, __global_pointer$
    la a0, __runtime_data_load_start
    la a1, __runtime_data_start
    la a2, __runtime_data_end
1:
    beq a1, a2, 2f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 1b
2:
    la a0, __runtime_data_bss_start
    la a1, __runtime_data_bss_end
3:
    beq a0, a1, 4f
    sw zero, 0(a0)
    addi a0, a0, 4
    j 3b
4:
    la a0, __runtime_isr_load_start
    la a1, __runtime_isr_start
    la a2, __runtime_isr_end
5:
    beq a1, a2, 6f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 5b
6:
    la a0, __runtime_critical_data_load_start
    la a1, __runtime_critical_data_start
    la a2, __runtime_critical_data_end
7:
    beq a1, a2, 8f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 7b
8:
    la a0, __runtime_critical_bss_start
    la a1, __runtime_critical_bss_end
9:
    beq a0, a1, 10f
    sw zero, 0(a0)
    addi a0, a0, 4
    j 9b
10:
    la a0, __runtime_dma_data_load_start
    la a1, __runtime_dma_data_start
    la a2, __runtime_dma_data_end
11:
    beq a1, a2, 12f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 11b
12:
    la a0, __runtime_dma_bss_start
    la a1, __runtime_dma_bss_end
13:
    beq a0, a1, 14f
    sw zero, 0(a0)
    addi a0, a0, 4
    j 13b
14:
    la a0, __runtime_hot_text_load_start
    la a1, __runtime_hot_text_start
    la a2, __runtime_hot_text_end
15:
    beq a1, a2, 16f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 15b
16:
    call _runtime_stack_bootstrap
    fence.i
    la t0, _start_trap
    ori t0, t0, 3
    csrw mtvec, t0
    la t0, _runtime_mtvt
    csrw {mtvt}, t0
    .option pop
    call _runtime_enable_fpu
    tail runtime_main
    // Only the facts-defined boot hart runs the runtime from here: a
    // secondary hart's boot word names only its own generated entry
    // (`_runtime_start_h<N>`, below) or zero, so a stale address that ROM
    // hands to another hart stops here, before the hart touches data, the
    // stacks or any peripheral. It spins: the handoff scan decodes no `wfi`.
99:
    j 99b
    .size _runtime_start, . - _runtime_start

"#,
    boot_hart = const crate::BOOT_HART,
    mtvt = const oer_riscv::clic::MTVT,
);

// A core with the F extension (`riscv32imafc`) starts its task code with the
// FPU on and a clear rounding/exception state; the trap entries turn it off
// for handlers. A core without it (`riscv32imac`) has no FPU state, and
// `fscsr` would be an illegal instruction.
#[cfg(target_feature = "f")]
core::arch::global_asm!(
    r#"
    .section .text.runtime_fpu, "ax", @progbits
    .balign 4
    .global _runtime_enable_fpu
    .type _runtime_enable_fpu, @function
_runtime_enable_fpu:
    li t0, {fs}
    csrrs zero, mstatus, t0
    .option push
    .option arch, +f
    fscsr zero
    .option pop
    ret
    .size _runtime_enable_fpu, . - _runtime_enable_fpu
"#,
    fs = const oer_riscv::machine::status::FS_MASK,
);

#[cfg(not(target_feature = "f"))]
core::arch::global_asm!(
    r#"
    .section .text.runtime_fpu, "ax", @progbits
    .balign 4
    .global _runtime_enable_fpu
    .type _runtime_enable_fpu, @function
_runtime_enable_fpu:
    ret
    .size _runtime_enable_fpu, . - _runtime_enable_fpu
"#
);

// A secondary hart's entry, instantiated once per secondary hart from the chip
// facts (`secondary.s`, after the PMA program it expands). The ROM's `main`
// path jumps here with interrupts enabled, its own trap vector and stack. The
// entry touches no stack and no data but the SRAM handshake until it installs
// its own: it masks interrupts with its first instruction, installs its
// 64-byte aligned fault stub (keeping the inherited vector mode), reports
// ENTERED and waits for the boot hart's ACK, which follows the boot word's
// retirement. It then refuses any PMP entry, disables every PMA entry and
// programs the chip's chain with a read-back of each write, synchronizes its
// instruction fetch, installs `gp`, its PSRAM task stack and the FPU, and
// tail-calls its Rust root. It never initializes data or BSS: the boot hart
// did. Any fault stores its kind and observations, then FAULT, and waits.
// The section is not kept: an image that never launches the hart links none
// of it.
#[cfg(chip_vectors)]
core::arch::global_asm!(
    r#"
    .macro SECONDARY_ENTRY hart
    .pushsection .rwtext.secondary_h\hart, "ax", @progbits
    .balign 64
    .global _runtime_secondary_fault_h\hart
    .type _runtime_secondary_fault_h\hart, @function
_runtime_secondary_fault_h\hart:
    li a0, {kind_exception}
    csrr a1, mcause
    csrr t0, mepc
    csrr t1, mtval
    .size _runtime_secondary_fault_h\hart, . - _runtime_secondary_fault_h\hart
    .global _runtime_secondary_report_h\hart
    .type _runtime_secondary_report_h\hart, @function
_runtime_secondary_report_h\hart:
    la t2, {handshake}
    sw a0, {kind}(t2)
    sw a1, {cause}(t2)
    sw t0, {pc}(t2)
    sw t1, {value}(t2)
    fence w, w
    li t3, {fault}
    sw t3, {ready}(t2)
1:
    wfi
    j 1b
    .size _runtime_secondary_report_h\hart, . - _runtime_secondary_report_h\hart

    .balign 4
    .global _runtime_start_h\hart
    .type _runtime_start_h\hart, @function
_runtime_start_h\hart:
    csrrci zero, mstatus, {mie}
    csrr t0, mtvec
    andi t0, t0, 3
    la t1, _runtime_secondary_fault_h\hart
    or t1, t1, t0
    csrw mtvec, t1
    la t2, {handshake}
    li t3, {entered_value}
    sw t3, {entered}(t2)
    li t4, {ack_value}
2:
    lw t3, {ack}(t2)
    bne t3, t4, 2b
    fence r, iorw
    li a0, {kind_pmp}
    li t0, 0
    li a1, 0
    csrr t1, pmpcfg0
    bne t1, t0, _runtime_secondary_report_h\hart
    li a1, 1
    csrr t1, pmpcfg1
    bne t1, t0, _runtime_secondary_report_h\hart
    li a1, 2
    csrr t1, pmpcfg2
    bne t1, t0, _runtime_secondary_report_h\hart
    li a1, 3
    csrr t1, pmpcfg3
    bne t1, t0, _runtime_secondary_report_h\hart
    PMA_PROGRAM _runtime_secondary_report_h\hart, {kind_pma}
    fence.i
    .option push
    .option norelax
    la gp, __global_pointer$
    .option pop
    la sp, __runtime_cpu\hart\()_task_stack_top
    call _runtime_enable_fpu
    tail runtime_secondary_main_h\hart
    .size _runtime_start_h\hart, . - _runtime_start_h\hart
    .popsection
    .endm
"#,
    include_str!(concat!(env!("OUT_DIR"), "/secondary.s")),
    handshake = sym crate::secondary::HANDSHAKE,
    mie = const oer_riscv::machine::status::MIE,
    entered = const core::mem::offset_of!(crate::secondary::Handshake, entered),
    ack = const core::mem::offset_of!(crate::secondary::Handshake, ack),
    ready = const core::mem::offset_of!(crate::secondary::Handshake, ready),
    kind = const core::mem::offset_of!(crate::secondary::Handshake, kind),
    cause = const core::mem::offset_of!(crate::secondary::Handshake, cause),
    pc = const core::mem::offset_of!(crate::secondary::Handshake, pc),
    value = const core::mem::offset_of!(crate::secondary::Handshake, value),
    entered_value = const crate::secondary::ENTERED,
    ack_value = const crate::secondary::ACK,
    fault = const crate::secondary::FAULT,
    kind_exception = const crate::secondary::KIND_EXCEPTION,
    kind_pmp = const crate::secondary::KIND_PMP,
    kind_pma = const crate::secondary::KIND_PMA,
);

// Each secondary hart's entry marker, its one `SecondaryEntry` mint and its
// Rust root, generated from the chip's hart geometry.
#[cfg(chip_vectors)]
include!(concat!(env!("OUT_DIR"), "/secondary.rs"));
