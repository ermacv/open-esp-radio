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
"#);
