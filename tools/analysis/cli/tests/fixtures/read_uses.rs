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

function unused_read
    lui t0, 0x20107
    lw t1, 0(t0)
    ret
end unused_read

function branch_read
    lui t0, 0x20107
    lw t1, 0(t0)
    bnez t1, 2f
    li a0, 0
    ret
2:  li a0, 1
    ret
end branch_read

function preserve_rmw
    lui t0, 0x20107
    lw t1, 0(t0)
    andi t1, t1, -2
    ori t1, t1, 1
    sw t1, 0(t0)
    ret
end preserve_rmw

function reset_write
    lui t0, 0x20107
    sw zero, 0(t0)
    ret
end reset_write

function nonreset_write
    lui t0, 0x20107
    li t1, 1
    sw t1, 0(t0)
    ret
end nonreset_write

function shifted_branch
    lui t0, 0x20107
    lw t1, 0(t0)
    srli t1, t1, 4
    andi t1, t1, 1
    bnez t1, 2f
    ret
2:  ret
end shifted_branch

function arithmetic_branch
    lui t0, 0x20107
    lw t1, 0(t0)
    add t1, t1, a0
    bnez t1, 2f
    ret
2:  ret
end arithmetic_branch

function load_address
    lui t0, 0x20107
    lw t1, 0(t0)
    lw t2, 0(t1)
    ret
end load_address

function call_argument
    lui t0, 0x20107
    lw a0, 0(t0)
    call external
    ret
end call_argument

function return_value
    lui t0, 0x20107
    lw a0, 0(t0)
    ret
end return_value

function store_other
    lui t0, 0x20107
    lw t1, 0(t0)
    sw t1, 4(t0)
    ret
end store_other

function moved_bit
    lui t0, 0x20107
    lw t1, 0(t0)
    andi t1, t1, 1
    slli t1, t1, 1
    sw t1, 0(t0)
    ret
end moved_bit

function inverted_bit
    lui t0, 0x20107
    lw t1, 0(t0)
    xori t1, t1, 1
    sw t1, 0(t0)
    ret
end inverted_bit

function partial_function
    lui t0, 0x20107
    lw t1, 0(t0)
    jr a2
end partial_function

function stack_spill
    lui t0, 0x20107
    lw t1, 0(t0)
    sw t1, -4(sp)
    ret
end stack_spill

function indirect_target
    lui t0, 0x20107
    lw t1, 0(t0)
    jr t1
end indirect_target

function unsupported_operation
    lui t0, 0x20107
    lw t1, 0(t0)
    ecall
    ret
end unsupported_operation

function signed_byte
    lui t0, 0x20107
    lb a0, 0(t0)
    ret
end signed_byte

function repeated_loads
    lui t0, 0x20107
    lw t1, 0(t0)
    lw t2, 0(t0)
    bnez t2, 2f
    ret
2:  ret
end repeated_loads

function used_and_spilled
    lui t0, 0x20107
    lw a0, 0(t0)
    sw a0, -4(sp)
    ret
end used_and_spilled

function variable_and
    lui t0, 0x20107
    lw t1, 0(t0)
    and t1, t1, a0
    andi t1, t1, 1
    bnez t1, 2f
    ret
2:  ret
end variable_and

function lost_join
    lui t0, 0x20107
    lw t1, 0(t0)
    beqz a2, 2f
    mv t1, a3
2:  bnez t1, 3f
    ret
3:  ret
end lost_join

function dispatch_index
    lui t0, 0x20107
    lw t1, 0(t0)
    andi t1, t1, 3
    slli t1, t1, 2
    lui t2, %hi(read_table)
    addi t2, t2, %lo(read_table)
    add t1, t1, t2
    lw t1, 0(t1)
    jr t1
2:  ret
3:  ret
4:  ret
5:  ret
end dispatch_index
.section .rodata.read_table,"a",@progbits
.p2align 2
.type read_table,@object
read_table:
    .word 2b, 3b, 4b, 5b
.size read_table, .-read_table
"#);
