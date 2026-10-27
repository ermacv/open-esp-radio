// Probe-owned trap ABI. IRQ storage is below task storage, both in SRAM.
// Frame operands come from this package's TrapFrame. The initial SP/mscratch
// exchanges precede every memory access; nested entries stay on the IRQ stack.
    .section .trap.facts, "ax", @progbits
    .option push
    .option norelax
    .option norvc

    // Swap before the first memory access. The second CSR exchange keeps
    // the interrupted t0 in mscratch and gives t0 the other stack pointer:
    //
    // outer:  sp = IRQ top, t0 = task sp
    // nested: sp = task sp, t0 = interrupted IRQ sp
    //
    // The unsigned comparison distinguishes the lower IRQ stack from the task stack. A nested entry
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
    .global _facts_start_trap
    .type _facts_start_trap, @function
_facts_start_trap:
    TRAP_ENTER
    mv a0, sp
    call facts_exception
3:
    j 3b
    .size _facts_start_trap, . - _facts_start_trap

    // One hardware-vector entry: enter, turn the triggers on when they were
    // on in what the trap interrupted (MTE = MPTE), admit nesting (the CLIC
    // lets only a higher level in), call facts_irq directly, exit. The
    // exception entry leaves triggers off while recording the exception.
    .balign 4
    .global _facts_irq
    .type _facts_irq, @function
_facts_irq:
    TRAP_ENTER
    csrr t1, {tcontrol}
    srli t1, t1, {mpte_shift}
    andi t1, t1, {mte}
    csrs {tcontrol}, t1
    csrsi mstatus, {mie}
    call facts_irq
    j _facts_irq_exit
    .size _facts_irq, . - _facts_irq

    .balign 4
    .global _facts_irq_exit
    .type _facts_irq_exit, @function
_facts_irq_exit:
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
    .size _facts_irq_exit, . - _facts_irq_exit

    // The probe's hardware-vector table (generated): its words from a
    // aligned boundary, the word of line n at 4n. Line zero has no entry
    // (exceptions use mtvec); other lines share the entry above. The
    // section holds the tables alone, so the first table's alignment is the
    // section's and `_facts_mtvt` its first word.
    .section .trap.mtvt, "a", @progbits
    .global _facts_mtvt
    .type _facts_mtvt, @object
_facts_mtvt:
    FACTS_VECTOR_TABLE
    .size _facts_mtvt, . - _facts_mtvt

    .option pop
