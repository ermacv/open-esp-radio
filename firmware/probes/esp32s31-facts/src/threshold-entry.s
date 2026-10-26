// Controlled interrupt-admission entries, C ABI:
// a0 = exclusive [u32; 6] output, a1 = new threshold.
// MIE is masked at entry/exit; the trap ABI preserves all caller registers.
// A pending interrupt may win BEFORE the threshold write. Its saved mepc
// identifies that invalid trial; do not count it as successful masking.
.section .text.facts_threshold_entry,"ax"
.option push
.option norvc
.option norelax
.macro THRESHOLD_ENTRY variant, nops, readback
.balign 4
.global facts_threshold_\variant
.type facts_threshold_\variant, @function
facts_threshold_\variant:
    li t1, -1
    la t3, 2f
    la t4, 3f
    la t5, 4f
    csrr t0, mcycle
    csrsi mstatus, {mie}
2:  csrw {mintthresh}, a1
3:
    .rept \nops
    nop
    .endr
    .if \readback
    csrr t1, {mintthresh}
    .endif
    csrr t2, mcycle
4:  csrci mstatus, {mie}
    sw t0, 0(a0)
    sw t2, 4(a0)
    sw t3, 8(a0)
    sw t4, 12(a0)
    sw t5, 16(a0)
    sw t1, 20(a0)
    ret
.size facts_threshold_\variant, . - facts_threshold_\variant
.endm
THRESHOLD_ENTRY 0, 0, 0
THRESHOLD_ENTRY 1, 1, 0
THRESHOLD_ENTRY 2, 2, 0
THRESHOLD_ENTRY 3, 3, 0
THRESHOLD_ENTRY 4, 0, 1
THRESHOLD_ENTRY 5, 1, 1
THRESHOLD_ENTRY 6, 2, 1
THRESHOLD_ENTRY 7, 3, 1
.option pop
