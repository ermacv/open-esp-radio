#![no_std]
core::arch::global_asm!(r#"
.option norvc
.option norelax
.section .text.unknown_stack,"ax",@progbits
.globl unknown_stack
.type unknown_stack,@function
unknown_stack:
addi sp,sp,-16
mul t1,a0,a1
sw t1,0(sp)
lw t1,0(sp)
lui t0,0x20107
sw t1,0(t0)
addi sp,sp,16
ret
.size unknown_stack,.-unknown_stack
"#);
