OUTPUT_ARCH(riscv)
ENTRY(_start)
EXTERN(_start);
INCLUDE "bootstrap/memory.x"
INCLUDE "bootstrap/sections.x"
