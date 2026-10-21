#![no_std]

core::arch::global_asm!(r#"
.option norvc
.option norelax
.macro function name
.section .text.\name,"ax",@progbits
.globl \name
.type \name,@function
\name:
.endm
.macro end name
.size \name, .-\name
.endm

function stale_a3
    addi sp, sp, -16
    sw ra, 12(sp)
    lui a3, 0x20107
    lw a3, 0(a3)
    call ignores_arguments
    li a0, 0
    li a1, 0
    lw ra, 12(sp)
    addi sp, sp, 16
    ret
end stale_a3

function ignores_arguments
    li a0, 0
    li a1, 0
    ret
end ignores_arguments

// Callers clear both physical result registers when they discard the result.
.macro caller name, reg, target
function \name
    lui t0, 0x20107
    lw \reg, 0(t0)
    call \target
    li a0, 0
    li a1, 0
    ret
end \name
.endm

caller a3_ignored, a3, consumes_a0
caller a0_consumed, a0, consumes_a0
caller a5_overwritten, a5, overwrites_a5
caller result_discarded, a0, identity
caller recursive_arguments, a3, recursive_a
caller self_recursive_arguments, a5, self_recursive
caller partial_arguments, a5, partial
caller external_arguments, a3, external
caller nested_consumption, a0, nested_consumer
caller stored_argument, a0, store_argument
caller address_argument, a0, address_argument_callee
caller tail_unknown_arguments, a3, tail_unknown

function consumes_a0
    andi t0, a0, 16
    bnez t0, 2f
2:  li a0, 0
    li a1, 0
    ret
end consumes_a0

function overwrites_a5
    li a5, 17
    bnez a5, 2f
2:  li a0, 0
    li a1, 0
    ret
end overwrites_a5

function identity
    ret
end identity

function shifted_result
    srli a1, a0, 4
    andi a1, a1, 1
    li a0, 0
    ret
end shifted_result

function inverted_result
    xori a0, a0, -1
    ret
end inverted_result

function nested_consumer
    call consumes_a0
    li a0, 0
    li a1, 0
    ret
end nested_consumer

function tail_identity
    tail identity
end tail_identity

function tail_unknown
    tail external
end tail_unknown

.macro branching_caller name, target, result, mask
function \name
    lui t0, 0x20107
    lw a0, 0(t0)
    call \target
    andi t0, \result, \mask
    bnez t0, 2f
2:  li a0, 0
    li a1, 0
    ret
end \name
.endm
branching_caller result_branch, identity, a0, 16
branching_caller tail_result_branch, tail_identity, a0, 16
branching_caller high_result_branch, shifted_result, a1, 1
branching_caller may_result_branch, joined_result, a0, 1

function joined_result
    beqz a2, 2f
    srli a0, a0, 1
    ret
2:  ret
end joined_result

caller may_consumption, a0, may_consumer
function may_consumer
    add a0, a0, a1
    bnez a0, 2f
2:  li a0, 0
    li a1, 0
    ret
end may_consumer

// Caller arithmetic also demotes an exact callee dependency to May.
function may_caller_consumption
    lui t0, 0x20107
    lw a0, 0(t0)
    add a0, a0, a1
    call consumes_a0
    li a0, 0
    li a1, 0
    ret
end may_caller_consumption

.macro storing_caller name, target
function \name
    lui t0, 0x20107
    lw a0, 0(t0)
    call \target
    lui t0, 0x20107
    sw a0, 0(t0)
    li a0, 0
    li a1, 0
    ret
end \name
.endm
storing_caller identity_store, identity
storing_caller inverted_store, inverted_result

function recursive_a
    call recursive_b
    li a0, 0
    li a1, 0
    ret
end recursive_a
function recursive_b
    call recursive_a
    li a0, 0
    li a1, 0
    ret
end recursive_b
function self_recursive
    call self_recursive
    li a0, 0
    li a1, 0
    ret
end self_recursive

function partial
    jr a2
end partial

function indirect_arguments
    lui t0, 0x20107
    lw a3, 0(t0)
    jalr ra, a2, 0
    li a0, 0
    li a1, 0
    ret
end indirect_arguments

function store_argument
    lui t0, 0x20107
    andi a0, a0, 4
    sw a0, 4(t0)
    li a0, 0
    li a1, 0
    ret
end store_argument

function address_argument_callee
    lw t0, 0(a0)
    li a0, 0
    li a1, 0
    ret
end address_argument_callee
"#);
