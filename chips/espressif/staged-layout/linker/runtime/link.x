OUTPUT_ARCH(riscv)
ENTRY(_runtime_start)

/* Minimal esp-riscv-rt integration. Runtime has its own entry and its data/BSS
   are initialized by the bootstrap, but the normal vectored interrupt stubs
   and esp-hal default handlers are still used after the handoff. */
EXTERN(_default_abort);
EXTERN(_default_start_trap);
PROVIDE(abort = _default_abort);
PROVIDE(_pre_init_trap = _default_abort);
PROVIDE(_start_trap = _default_start_trap);
PROVIDE(_runtime_mtvt_table = _mtvt_table);
PROVIDE(ExceptionHandler = abort);
PROVIDE(DefaultHandler = EspDefaultHandler);
PROVIDE(_start_DefaultHandler_trap = _start_trap);
PROVIDE(_max_hart_id = HART_COUNT - 1);
PROVIDE(main = runtime_main);
PROVIDE(hal_main = runtime_main);

/* riscv-rt's unused normal-reset path remains in the same LTO object as its
   vector table. Satisfy its initialization aliases without letting it touch
   the already relocated runtime image. */
__sdata = 0;
__edata = 0;
__sidata = 0;
PROVIDE(interrupt0 = DefaultHandler);
PROVIDE(interrupt1 = DefaultHandler);
PROVIDE(interrupt2 = DefaultHandler);
PROVIDE(interrupt3 = DefaultHandler);
PROVIDE(interrupt4 = DefaultHandler);
PROVIDE(interrupt5 = DefaultHandler);
PROVIDE(interrupt6 = DefaultHandler);
PROVIDE(interrupt7 = DefaultHandler);
PROVIDE(interrupt8 = DefaultHandler);
PROVIDE(interrupt9 = DefaultHandler);
PROVIDE(interrupt10 = DefaultHandler);
PROVIDE(interrupt11 = DefaultHandler);
PROVIDE(interrupt12 = DefaultHandler);
PROVIDE(interrupt13 = DefaultHandler);
PROVIDE(interrupt14 = DefaultHandler);
PROVIDE(interrupt15 = DefaultHandler);
PROVIDE(interrupt16 = DefaultHandler);
PROVIDE(interrupt17 = DefaultHandler);
PROVIDE(interrupt18 = DefaultHandler);
PROVIDE(interrupt19 = DefaultHandler);
PROVIDE(interrupt20 = DefaultHandler);
PROVIDE(interrupt21 = DefaultHandler);
PROVIDE(interrupt22 = DefaultHandler);
PROVIDE(interrupt23 = DefaultHandler);
PROVIDE(interrupt24 = DefaultHandler);
PROVIDE(interrupt25 = DefaultHandler);
PROVIDE(interrupt26 = DefaultHandler);
PROVIDE(interrupt27 = DefaultHandler);
PROVIDE(interrupt28 = DefaultHandler);
PROVIDE(interrupt29 = DefaultHandler);
PROVIDE(interrupt30 = DefaultHandler);
PROVIDE(interrupt31 = DefaultHandler);
INCLUDE "device.x"

INCLUDE "runtime/memory.x"
INCLUDE "runtime/sections.x"
