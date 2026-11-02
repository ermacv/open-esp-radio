//! Reset runs on the boot hart only, with interrupts masked. The ESP image
//! loader places initialized SRAM segments at their linked addresses; reset
//! clears only BSS, preserving noinit and retained memory. All bounds and
//! the stack placement are supplied by the bootstrap linker script.
core::arch::global_asm!(
    r#"
    .section .init, "ax", @progbits
    .balign 4
    .global _start
    .type _start, @function
_start:
    csrci mstatus, {mie}
    csrr t0, mhartid
    li t1, {boot_hart}
    bne t0, t1, 9f
    .option push
    .option norelax
    la gp, __global_pointer$
    la sp, _stack_start
    .option pop
    la t0, _bootstrap_trap
    csrw mtvec, t0
    la a0, _bss_start
    la a1, _bss_end
1:
    beq a0, a1, 2f
    sw zero, 0(a0)
    addi a0, a0, 4
    j 1b
2:
    call _bootstrap_enable_fpu
    fence.i
    tail bootstrap_main
9:
    wfi
    j 9b
    .size _start, . - _start

    .section .trap, "ax", @progbits
    .balign 4
    .global _bootstrap_trap
    .type _bootstrap_trap, @function
_bootstrap_trap:
    csrci mstatus, {mie}
3:
    wfi
    j 3b
    .size _bootstrap_trap, . - _bootstrap_trap
"#,
    mie = const oer_riscv::machine::status::MIE,
    boot_hart = const oer_espressif_staged_layout::boot::BOOT_HART,
);

#[cfg(target_feature = "f")]
core::arch::global_asm!(
    r#"
    .section .init.fpu, "ax", @progbits
    .global _bootstrap_enable_fpu
    .type _bootstrap_enable_fpu, @function
_bootstrap_enable_fpu:
    li t0, {fs}
    csrrs zero, mstatus, t0
    .option push
    .option arch, +f
    fscsr zero
    .option pop
    ret
    .size _bootstrap_enable_fpu, . - _bootstrap_enable_fpu
"#,
    fs = const oer_riscv::machine::status::FS_MASK,
);
#[cfg(not(target_feature = "f"))]
core::arch::global_asm!(
    r#"
    .section .init.fpu, "ax", @progbits
    .global _bootstrap_enable_fpu
    .type _bootstrap_enable_fpu, @function
_bootstrap_enable_fpu:
    ret
    .size _bootstrap_enable_fpu, . - _bootstrap_enable_fpu
"#
);
