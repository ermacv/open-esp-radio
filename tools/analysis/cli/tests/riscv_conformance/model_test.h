// Blobray target model for the RISC-V architectural tests. The test ends by
// jumping to the executor's return sentinel, so an execution with a `return`
// goal completes there; the signature is the memory between
// `begin_signature` and `end_signature`.
#ifndef _COMPLIANCE_MODEL_H
#define _COMPLIANCE_MODEL_H

#define ALIGNMENT 2

#define RVMODEL_DATA_SECTION \
        .pushsection .tohost,"aw",@progbits; \
        .align 8; .global tohost; tohost: .dword 0; \
        .align 8; .global fromhost; fromhost: .dword 0; \
        .popsection; \
        .align 8; .global begin_regstate; begin_regstate: \
        .word 128; \
        .align 8; .global end_regstate; end_regstate: \
        .word 4;

#define RVMODEL_HALT \
        li t0, 0xfffffffe; \
        jr t0;

#define RVMODEL_BOOT

#define RVMODEL_DATA_BEGIN \
        .align 4; .global begin_signature; begin_signature:

#define RVMODEL_DATA_END \
        .align 4; .global end_signature; end_signature: \
        RVMODEL_DATA_SECTION

#define RVMODEL_IO_INIT
#define RVMODEL_IO_WRITE_STR(_R, _STR)
#define RVMODEL_IO_CHECK()
#define RVMODEL_IO_ASSERT_GPR_EQ(_S, _R, _I)
#define RVMODEL_IO_ASSERT_SFPR_EQ(_F, _R, _I)
#define RVMODEL_IO_ASSERT_DFPR_EQ(_D, _R, _I)
#define RVMODEL_SET_MSW_INT
#define RVMODEL_CLEAR_MSW_INT
#define RVMODEL_CLEAR_MTIMER_INT
#define RVMODEL_CLEAR_MEXT_INT

#endif
