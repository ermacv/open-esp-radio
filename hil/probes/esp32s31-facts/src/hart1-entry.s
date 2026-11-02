// Hart 1 entry for selections 232/233. The rev0 ROM jumps here with MIE set,
// ROM mtvec and ROM stack. Nothing below needs a stack until `call`.
// Word offsets match FACTS_HART1 in image/hart1_entry.rs.
.section .text.facts_hart1,"ax"
.option push
.option norvc
.balign 256
.global facts_hart1_trap
facts_hart1_trap:
    la t0, FACTS_HART1
    csrr t1, {mcause}
    sw t1, 125*4(t0)
    csrr t1, {mepc}
    sw t1, 126*4(t0)
    csrr t1, 0x343
    sw t1, 127*4(t0)
    fence w, w
    li t1, 0xfac7100f
    sw t1, 124*4(t0)
1:  j 1b

.balign 4
.global facts_hart1_entry
facts_hart1_entry:
    csrrci t2, {mstatus}, {mie}
    csrr t3, {mtvec}
    la t0, facts_hart1_trap
    andi t4, t3, 3
    or t0, t0, t4
    csrw {mtvec}, t0
    la t0, FACTS_HART1
    sw t2, 1*4(t0)
    sw t3, 2*4(t0)
    csrr t1, {mtvt}
    sw t1, 3*4(t0)
    csrr t1, {mscratch}
    sw t1, 4*4(t0)
    csrr t1, 0x7c1
    sw t1, 5*4(t0)
    csrr t1, 0x306
    sw t1, 6*4(t0)
    csrr t1, {mintthresh}
    sw t1, 7*4(t0)
    csrr t1, {mintstatus}
    sw t1, 8*4(t0)
    csrr t1, 0xf14
    sw t1, 9*4(t0)
    csrr t1, 0x304
    sw t1, 10*4(t0)
    csrr t1, 0x344
    sw t1, 11*4(t0)
    sw sp, 12*4(t0)
    sw ra, 13*4(t0)
    sw gp, 14*4(t0)
    csrr t1, {mcause}
    sw t1, 15*4(t0)
    csrr t1, 0x3a0
    sw t1, 16*4(t0)
    csrr t1, 0x3a1
    sw t1, 17*4(t0)
    csrr t1, 0x3a2
    sw t1, 18*4(t0)
    csrr t1, 0x3a3
    sw t1, 19*4(t0)
    csrr t1, 0x3b0
    sw t1, 20*4(t0)
    csrr t1, 0x3b1
    sw t1, 21*4(t0)
    csrr t1, 0x3b2
    sw t1, 22*4(t0)
    csrr t1, 0x3b3
    sw t1, 23*4(t0)
    csrr t1, 0x3b4
    sw t1, 24*4(t0)
    csrr t1, 0x3b5
    sw t1, 25*4(t0)
    csrr t1, 0x3b6
    sw t1, 26*4(t0)
    csrr t1, 0x3b7
    sw t1, 27*4(t0)
    csrr t1, 0x3b8
    sw t1, 28*4(t0)
    csrr t1, 0x3b9
    sw t1, 29*4(t0)
    csrr t1, 0x3ba
    sw t1, 30*4(t0)
    csrr t1, 0x3bb
    sw t1, 31*4(t0)
    csrr t1, 0x3bc
    sw t1, 32*4(t0)
    csrr t1, 0x3bd
    sw t1, 33*4(t0)
    csrr t1, 0x3be
    sw t1, 34*4(t0)
    csrr t1, 0x3bf
    sw t1, 35*4(t0)
    csrr t1, 0xbc0
    sw t1, 36*4(t0)
    csrr t1, 0xbc1
    sw t1, 37*4(t0)
    csrr t1, 0xbc2
    sw t1, 38*4(t0)
    csrr t1, 0xbc3
    sw t1, 39*4(t0)
    csrr t1, 0xbc4
    sw t1, 40*4(t0)
    csrr t1, 0xbc5
    sw t1, 41*4(t0)
    csrr t1, 0xbc6
    sw t1, 42*4(t0)
    csrr t1, 0xbc7
    sw t1, 43*4(t0)
    csrr t1, 0xbc8
    sw t1, 44*4(t0)
    csrr t1, 0xbc9
    sw t1, 45*4(t0)
    csrr t1, 0xbca
    sw t1, 46*4(t0)
    csrr t1, 0xbcb
    sw t1, 47*4(t0)
    csrr t1, 0xbcc
    sw t1, 48*4(t0)
    csrr t1, 0xbcd
    sw t1, 49*4(t0)
    csrr t1, 0xbce
    sw t1, 50*4(t0)
    csrr t1, 0xbcf
    sw t1, 51*4(t0)
    csrr t1, 0xbd0
    sw t1, 52*4(t0)
    csrr t1, 0xbd1
    sw t1, 53*4(t0)
    csrr t1, 0xbd2
    sw t1, 54*4(t0)
    csrr t1, 0xbd3
    sw t1, 55*4(t0)
    csrr t1, 0xbd4
    sw t1, 56*4(t0)
    csrr t1, 0xbd5
    sw t1, 57*4(t0)
    csrr t1, 0xbd6
    sw t1, 58*4(t0)
    csrr t1, 0xbd7
    sw t1, 59*4(t0)
    csrr t1, 0xbd8
    sw t1, 60*4(t0)
    csrr t1, 0xbd9
    sw t1, 61*4(t0)
    csrr t1, 0xbda
    sw t1, 62*4(t0)
    csrr t1, 0xbdb
    sw t1, 63*4(t0)
    csrr t1, 0xbdc
    sw t1, 64*4(t0)
    csrr t1, 0xbdd
    sw t1, 65*4(t0)
    csrr t1, 0xbde
    sw t1, 66*4(t0)
    csrr t1, 0xbdf
    sw t1, 67*4(t0)
    csrr t1, {mtvec}
    sw t1, 68*4(t0)
    fence w, w
    li t1, 0xfac71001
    sw t1, 0(t0)
.option push
.option norelax
    la gp, __global_pointer$
.option pop
    la sp, __facts_hart1_stack_top
    call facts_hart1_main
2:  j 2b
.option pop
