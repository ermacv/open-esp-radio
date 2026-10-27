    // The runtime's traps: the exception entry `mtvec` names, one entry per
    // hart and CPU line in the hart's vector table (`mtvt`), and their one
    // checked exit. The runtime's build generates, from the chip facts' lines
    // and harts, the macros `RUNTIME_IRQ_ENTRIES` (an `IRQ_ENTRY hart, line`
    // for every line but 0 of every hart), plus typed Rust vector tables.
    // This text holds what every assembly entry shares and invokes the macro.
    // `stacks.rs` assembles the generated text and this one with the operands:
    //   {mtvt}             sym of the immutable Rust MTVT static
    //   {frame_bytes}      size_of!(TrapFrame)
    //   frame_* operands   offset_of!(TrapFrame, field), named for its field
    //   {fs}               mstatus.FS
    //   {mie}              mstatus.MIE
    //   {mintthresh}       the CLIC threshold CSR
    //   {tcontrol}         the trigger control CSR
    //   {mte}              tcontrol.MTE
    //   {mpte_shift}       the bits from tcontrol.MPTE down to tcontrol.MTE
    //
    // Every entry swaps `sp` with `mscratch` before its first memory access
    // and saves the interrupted caller registers, `mstatus`, `mepc`,
    // `mcause`, the threshold and `tcontrol` in a TrapFrame on the hart's
    // SRAM interrupt stack before it admits a nested interrupt; it
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
    // Frame size and every member offset come from TrapFrame through const
    // operands; the assembly has no independent copy of its Rust layout.
    .section .trap.runtime, "ax", @progbits
    .option push
    .option norelax
    .option norvc

    // The static is defined in the same codegen unit as this export.
    .global _runtime_mtvt
    .type _runtime_mtvt, @object
    .set _runtime_mtvt, {mtvt}

    .balign 4
    .global _runtime_stack_bootstrap
    .type _runtime_stack_bootstrap, @function
_runtime_stack_bootstrap:
    RUNTIME_BOOT_TASK_BOTTOM t0
    addi t0, t0, 256
    RUNTIME_BOOT_TASK_TOP t1
    addi t1, t1, -256
    li t2, 0xa55aa55a
1:
    bgeu t0, t1, 2f
    sw t2, 0(t0)
    addi t0, t0, 4
    j 1b
2:
    RUNTIME_BOOT_TASK_TOP sp
    RUNTIME_BOOT_IRQ_TOP t0
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
    addi sp, sp, -{frame_bytes}
    sw t1, {frame_t1}(sp)
    sw zero, {frame_outermost}(sp)
    // The interrupted handler's sp is just above this frame.
    addi t1, sp, {frame_bytes}
    sw t1, {frame_sp}(sp)
    j 91f

90:
    addi sp, sp, -{frame_bytes}
    sw t1, {frame_t1}(sp)
    li t1, 1
    sw t1, {frame_outermost}(sp)
    sw t0, {frame_sp}(sp)

91:
    sw ra, {frame_ra}(sp)
    // Both paths saved t1 before using it to recover the interrupted t0.
    csrr t1, mscratch
    sw t1, {frame_t0}(sp)
    sw t2, {frame_t2}(sp)
    sw t3, {frame_t3}(sp)
    sw t4, {frame_t4}(sp)
    sw t5, {frame_t5}(sp)
    sw t6, {frame_t6}(sp)
    sw a0, {frame_a0}(sp)
    sw a1, {frame_a1}(sp)
    sw a2, {frame_a2}(sp)
    sw a3, {frame_a3}(sp)
    sw a4, {frame_a4}(sp)
    sw a5, {frame_a5}(sp)
    sw a6, {frame_a6}(sp)
    sw a7, {frame_a7}(sp)
    csrr t1, mstatus
    sw t1, {frame_mstatus}(sp)
    csrr t1, mepc
    sw t1, {frame_mepc}(sp)
    csrr t1, mcause
    sw t1, {frame_mcause}(sp)
    csrr t1, {mintthresh}
    sw t1, {frame_mintthresh}(sp)
    csrr t1, {tcontrol}
    sw t1, {frame_tcontrol}(sp)
    li t1, {fs}
    csrc mstatus, t1
    csrw mscratch, t0
    .endm

    .macro TRAP_RESTORE_REGISTERS
    lw ra, {frame_ra}(sp)
    lw t0, {frame_t0}(sp)
    lw t1, {frame_t1}(sp)
    lw t2, {frame_t2}(sp)
    lw t3, {frame_t3}(sp)
    lw t4, {frame_t4}(sp)
    lw t5, {frame_t5}(sp)
    lw t6, {frame_t6}(sp)
    lw a0, {frame_a0}(sp)
    lw a1, {frame_a1}(sp)
    lw a2, {frame_a2}(sp)
    lw a3, {frame_a3}(sp)
    lw a4, {frame_a4}(sp)
    lw a5, {frame_a5}(sp)
    lw a6, {frame_a6}(sp)
    lw a7, {frame_a7}(sp)
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

    RUNTIME_IRQ_ENTRIES

    .balign 4
    .global _runtime_irq_exit
    .type _runtime_irq_exit, @function
_runtime_irq_exit:
    csrci mstatus, {mie}
    lw t0, {frame_mcause}(sp)
    csrw mcause, t0
    lw t0, {frame_mepc}(sp)
    csrw mepc, t0
    lw t0, {frame_tcontrol}(sp)
    csrw {tcontrol}, t0
    lw t0, {frame_mstatus}(sp)
    csrw mstatus, t0
    lw t0, {frame_mintthresh}(sp)
    csrw {mintthresh}, t0
    lw t0, {frame_outermost}(sp)
    beqz t0, 92f

    // The outermost return restores the task stack and arms mscratch with
    // the IRQ stack top for the next first-level trap.
    TRAP_RESTORE_REGISTERS
    addi sp, sp, {frame_bytes}
    csrrw sp, mscratch, sp
    mret

92:
    // A nested return resumes the interrupted handler on its SRAM stack;
    // mscratch stays the task sp.
    TRAP_RESTORE_REGISTERS
    addi sp, sp, {frame_bytes}
    mret
    .size _runtime_irq_exit, . - _runtime_irq_exit

    .option pop
