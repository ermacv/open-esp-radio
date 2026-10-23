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
.macro begin name
function \name
    addi sp, sp, -16
    sw ra, 12(sp)
    li a0, 7
    li a1, 1
    li a2, 3
    call read_word
.endm
.macro finish name
    li a0, 0
    li a1, 0
    lw ra, 12(sp)
    addi sp, sp, 16
    ret
end \name
.endm

begin branch_result
    bnez a0, 2f
    nop
2:
finish branch_result

begin unchanged_result
    mv a3, a0
    call write_word
finish unchanged_result

begin opaque_result
    mv a3, a0
    li a0, 0
    call write_external
finish opaque_result

begin masked_result
    andi a3, a0, -2
    call write_word
finish masked_result

begin shifted_result
    slli a3, a0, 4
    call write_word
finish shifted_result

begin right_shifted_result
    srli a3, a0, 4
    call write_word
finish right_shifted_result

begin mixed_result
    add a3, a0, s1
    call write_word
finish mixed_result

begin inverted_result
    xori a3, a0, 1
    call write_word
finish inverted_result

begin printed_result
    call print_word
finish printed_result

begin discarded_result
finish discarded_result

begin overwritten_result
    li a0, 0
    mv a3, a0
    call write_word
finish overwritten_result

begin spilled_result
    sw a0, 8(sp)
    lw a3, 8(sp)
    call write_word
finish spilled_result

begin twice_result
    mv s1, a0
    call read_word
    add a3, a0, s1
    call write_word
finish twice_result

function double_result
    addi sp, sp, -16
    sw ra, 12(sp)
    call read_double
    bnez a1, 2f
    nop
2:
finish double_result

function truncated_result
    call read_word
end truncated_result

function tail_result
    tail read_word
end tail_result

function write_word
    lui t0, 0x20107
    sw a3, 0(t0)
    ret
end write_word
"#);
