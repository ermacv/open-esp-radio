OUTPUT_ARCH(riscv)
ENTRY(_runtime_start)

/* Stage two's own runtime: its entry, its traps and vector tables
   (staged-runtime `vectors.s` and the entries and tables its build
   generates) and its default handlers. Each line's vector
   calls `__oer_interrupt_h<hart>_l<line>`: the interrupt table's handler of
   the entry bound to that line, else the runtime's unhandled-interrupt stop.
   An exception calls the image's `oer_exception_handler`, else the
   runtime's default stop. */
EXTERN(oer_unhandled_interrupt);
EXTERN(oer_default_exception);
EXTERN(_runtime_mtvt);
PROVIDE(oer_exception_handler = oer_default_exception);
/* Every line's default handler, which the staged runtime's build generates
   from the chip facts' lines and harts into its build directory, a library
   search path of every image that links the runtime. */
INCLUDE "oer-staged-runtime-vectors.x"

INCLUDE "runtime/memory.x"
INCLUDE "runtime/sections.x"
INCLUDE "oer-staged-runtime-stacks.x"
