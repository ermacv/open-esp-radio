// C ABI: a0 = exclusive [u32; 6], a1 = live shared FromCpu2, a2 = ceiling.
// MIE is masked at entry/exit. The source is NOT pending at entry.
// The typed Rust thunk performs the only raise store, without readback;
// its one fixed ret precedes the unrolled K NOPs. No raw MMIO lives here.
.section .text.facts_threshold_entry,"ax"
.option push
.option norvc
.option norelax
.macro THRESHOLD_ENTRY k, variant, nops, readback
.balign 4
.global facts_threshold_\k\()_\variant
.type facts_threshold_\k\()_\variant, @function
facts_threshold_\k\()_\variant:
    addi sp, sp, -32
    sw ra, 28(sp)
    sw s0, 0(sp)
    sw s1, 4(sp)
    sw s2, 8(sp)
    sw s3, 12(sp)
    sw s4, 16(sp)
    sw s5, 20(sp)
    sw s6, 24(sp)
    mv s0, a0
    mv s1, a2
    mv a0, a1
    li s3, -1
    la s4, 2f
    la s5, 3f
    la s6, 4f
    csrr s2, mcycle
    csrsi mstatus, {mie}
    call facts_threshold_raise
    .rept \k
    nop
    .endr
2:  csrw {mintthresh}, s1
3:
    .rept \nops
    nop
    .endr
    .if \readback
    csrr s3, {mintthresh}
    .endif
    csrr t0, mcycle
4:  csrci mstatus, {mie}
    sw s2, 0(s0)
    sw t0, 4(s0)
    sw s4, 8(s0)
    sw s5, 12(s0)
    sw s6, 16(s0)
    sw s3, 20(s0)
    lw ra, 28(sp)
    lw s0, 0(sp)
    lw s1, 4(sp)
    lw s2, 8(sp)
    lw s3, 12(sp)
    lw s4, 16(sp)
    lw s5, 20(sp)
    lw s6, 24(sp)
    addi sp, sp, 32
    ret
.size facts_threshold_\k\()_\variant, . - facts_threshold_\k\()_\variant
.endm
.irp k, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24
THRESHOLD_ENTRY \k, 0, 0, 0
THRESHOLD_ENTRY \k, 1, 1, 0
THRESHOLD_ENTRY \k, 2, 2, 0
THRESHOLD_ENTRY \k, 3, 3, 0
THRESHOLD_ENTRY \k, 4, 0, 1
THRESHOLD_ENTRY \k, 5, 1, 1
THRESHOLD_ENTRY \k, 6, 2, 1
THRESHOLD_ENTRY \k, 7, 3, 1
.endr
.section .rodata.facts_threshold_entries,"a"
.balign 4
.global _facts_threshold_entries
.type _facts_threshold_entries, @object
_facts_threshold_entries:
.irp k, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24
.word facts_threshold_\k\()_0, facts_threshold_\k\()_1
.word facts_threshold_\k\()_2, facts_threshold_\k\()_3
.word facts_threshold_\k\()_4, facts_threshold_\k\()_5
.word facts_threshold_\k\()_6, facts_threshold_\k\()_7
.endr
.size _facts_threshold_entries, . - _facts_threshold_entries
.option pop
