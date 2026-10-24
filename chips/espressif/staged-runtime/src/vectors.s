    // The runtime's traps: the exception entry `mtvec` names, one entry per
    // hart and CPU line in the hart's vector table (`mtvt`), and their one
    // checked exit. `stacks.rs` assembles this text with its operands:
    //   {fs}               mstatus.FS
    //   {mie}              mstatus.MIE
    //   {mintthresh}       the CLIC threshold CSR
    //   {harts}            the harts with an interrupt stack (1 or 2)
    //   {raise_threshold}  1 when clearing MIE alone does not mask the CLIC
    //   {tcontrol}         the trigger control CSR
    //   {mte}              tcontrol.MTE
    //   {mpte_shift}       the bits from tcontrol.MPTE down to tcontrol.MTE
    //
    // Every entry swaps `sp` with `mscratch` before its first memory access
    // and saves the interrupted caller registers, `mstatus`, `mepc`,
    // `mcause`, the threshold and `tcontrol` in a 96-byte frame on the
    // hart's SRAM interrupt stack before it admits a nested interrupt; it
    // turns the FPU off, since no trap saves the floating-point registers,
    // and calls its one handler directly. An interrupt's entry gives its
    // handler the triggers of the context it interrupted, which the trap
    // moved to `tcontrol.MPTE`, so the interrupt stack's guard fires in
    // handlers. The exit closes nesting first, restores `mcause` (its `mpil`
    // is the level `mret` returns to), `mepc`, `tcontrol` (its MPTE is the
    // MTE `mret` returns to), `mstatus` and the threshold from that frame,
    // then the registers and the stack, and returns. Nested traps thus keep
    // each level's trigger state: a trap overwrites MPTE, the frame does not.
    //
    // Frame: ra, t0..t6, a0..a7 at 0..60, the outermost flag at 64,
    // mstatus at 68, the interrupted sp at 72, mepc at 76, mcause at 80,
    // the threshold at 84, tcontrol at 88; 92..96 unused.
    .section .trap.runtime, "ax", @progbits
    .option push
    .option norelax
    .option norvc

    .balign 4
    .global _runtime_stack_bootstrap
    .type _runtime_stack_bootstrap, @function
_runtime_stack_bootstrap:
    la t0, __runtime_cpu0_task_stack_bottom
    addi t0, t0, 256
    la t1, __runtime_cpu0_task_stack_top
    addi t1, t1, -256
    li t2, 0xa55aa55a
1:
    bgeu t0, t1, 2f
    sw t2, 0(t0)
    addi t0, t0, 4
    j 1b
2:
    la sp, __runtime_cpu0_task_stack_top
    la t0, __runtime_cpu0_irq_stack_top
    csrw mscratch, t0
    ret
    .size _runtime_stack_bootstrap, . - _runtime_stack_bootstrap

    // Swap before the first memory access. The second CSR exchange keeps
    // the interrupted t0 in mscratch and gives t0 the other stack pointer:
    //
    // outer:  sp = IRQ top, t0 = task sp
    // nested: sp = task sp, t0 = interrupted IRQ sp
    //
    // The unsigned comparison tells internal SRAM from PSRAM. A nested entry
    // swaps sp and t0 back with register-only XORs; both paths then take an
    // aligned frame on the SRAM stack. While a handler runs, mscratch holds
    // the task sp, so deeper nesting goes the same way.
    .macro TRAP_ENTER
    csrrw sp, mscratch, sp
    csrrw t0, mscratch, t0
    bltu sp, t0, 90f

    xor sp, sp, t0
    xor t0, sp, t0
    xor sp, sp, t0
    addi sp, sp, -96
    sw t1, 8(sp)
    sw zero, 64(sp)
    // The interrupted handler's sp is just above this frame.
    addi t1, sp, 96
    sw t1, 72(sp)
    j 91f

90:
    addi sp, sp, -96
    sw t1, 8(sp)
    li t1, 1
    sw t1, 64(sp)
    sw t0, 72(sp)

91:
    sw ra, 0(sp)
    // Both paths saved t1 before using it to recover the interrupted t0.
    csrr t1, mscratch
    sw t1, 4(sp)
    sw t2, 12(sp)
    sw t3, 16(sp)
    sw t4, 20(sp)
    sw t5, 24(sp)
    sw t6, 28(sp)
    sw a0, 32(sp)
    sw a1, 36(sp)
    sw a2, 40(sp)
    sw a3, 44(sp)
    sw a4, 48(sp)
    sw a5, 52(sp)
    sw a6, 56(sp)
    sw a7, 60(sp)
    csrr t1, mstatus
    sw t1, 68(sp)
    csrr t1, mepc
    sw t1, 76(sp)
    csrr t1, mcause
    sw t1, 80(sp)
    csrr t1, {mintthresh}
    sw t1, 84(sp)
    csrr t1, {tcontrol}
    sw t1, 88(sp)
    li t1, {fs}
    csrc mstatus, t1
    csrw mscratch, t0
    .endm

    .macro TRAP_RESTORE_REGISTERS
    lw ra, 0(sp)
    lw t0, 4(sp)
    lw t1, 8(sp)
    lw t2, 12(sp)
    lw t3, 16(sp)
    lw t4, 20(sp)
    lw t5, 24(sp)
    lw t6, 28(sp)
    lw a0, 32(sp)
    lw a1, 36(sp)
    lw a2, 40(sp)
    lw a3, 44(sp)
    lw a4, 48(sp)
    lw a5, 52(sp)
    lw a6, 56(sp)
    lw a7, 60(sp)
    .endm

    // Exceptions enter at mtvec's base (CLIC mode: 64-byte aligned), as does
    // an interrupt of a non-vectored line, which the table never configures.
    // The handler records the fault and never returns.
    .balign 64
    .global _start_trap
    .type _start_trap, @function
_start_trap:
    TRAP_ENTER
    mv a0, sp
    call oer_exception_handler
3:
    j 3b
    .size _start_trap, . - _start_trap

    // One hardware-vector entry: enter, turn the triggers on when they were
    // on in what the trap interrupted (MTE = MPTE), admit nesting (the CLIC
    // lets only a higher level in), call the line's handler, exit. The
    // exception entry leaves them off: a store into the interrupt stack's
    // guard trapped to it, and its own frame lies below that guard.
    .macro IRQ_ENTRY hart, line
    .balign 4
    .global _runtime_irq_h\hart\()_l\line
    .type _runtime_irq_h\hart\()_l\line, @function
_runtime_irq_h\hart\()_l\line:
    TRAP_ENTER
    csrr t1, {tcontrol}
    srli t1, t1, {mpte_shift}
    andi t1, t1, {mte}
    csrs {tcontrol}, t1
    csrsi mstatus, {mie}
    call __oer_interrupt_h\hart\()_l\line
    j _runtime_irq_exit
    .size _runtime_irq_h\hart\()_l\line, . - _runtime_irq_h\hart\()_l\line
    .endm

    .macro HART_ENTRIES hart
    .irp line, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47
    IRQ_ENTRY \hart, \line
    .endr
    .endm

    HART_ENTRIES 0
    .if {harts} > 1
    HART_ENTRIES 1
    .endif

    .balign 4
    .global _runtime_irq_exit
    .type _runtime_irq_exit, @function
_runtime_irq_exit:
    .if {raise_threshold}
    // Mask the CLIC by its threshold before clearing MIE, which alone does
    // not mask every interrupt on this core.
    li t0, 0xff
    csrw {mintthresh}, t0
    .endif
    csrci mstatus, {mie}
    lw t0, 80(sp)
    csrw mcause, t0
    lw t0, 76(sp)
    csrw mepc, t0
    lw t0, 88(sp)
    csrw {tcontrol}, t0
    lw t0, 68(sp)
    csrw mstatus, t0
    lw t0, 84(sp)
    csrw {mintthresh}, t0
    lw t0, 64(sp)
    beqz t0, 92f

    // The outermost return restores the task stack and arms mscratch with
    // the IRQ stack top for the next first-level trap.
    TRAP_RESTORE_REGISTERS
    addi sp, sp, 96
    csrrw sp, mscratch, sp
    mret

92:
    // A nested return resumes the interrupted handler on its SRAM stack;
    // mscratch stays the task sp.
    TRAP_RESTORE_REGISTERS
    addi sp, sp, 96
    mret
    .size _runtime_irq_exit, . - _runtime_irq_exit

    // Each hart's hardware-vector table: 64 words from a 256-byte boundary,
    // the word of line n at 4n. Line zero has no entry (exceptions use
    // mtvec); lines 48 to 63 do not exist.
    .macro HART_TABLE hart
    .balign 256
    .word 0
    .irp line, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47
    .word _runtime_irq_h\hart\()_l\line
    .endr
    .fill 16, 4, 0
    .endm

    .section .trap.mtvt, "a", @progbits
    .balign 256
    .global _runtime_mtvt
    .type _runtime_mtvt, @object
_runtime_mtvt:
    HART_TABLE 0
    .if {harts} > 1
    HART_TABLE 1
    .endif
    .size _runtime_mtvt, . - _runtime_mtvt

    .option pop
