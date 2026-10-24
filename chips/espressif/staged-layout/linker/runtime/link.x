OUTPUT_ARCH(riscv)
ENTRY(_runtime_start)

/* Stage two's own runtime: its entry, its traps and vector tables
   (staged-runtime `vectors.s`) and its default handlers. Each line's vector
   calls `__oer_interrupt_h<hart>_l<line>`: the interrupt table's handler of
   the entry bound to that line, else the runtime's unhandled-interrupt stop.
   An exception calls the image's `oer_exception_handler`, else the
   runtime's default stop. */
EXTERN(oer_unhandled_interrupt);
EXTERN(oer_default_exception);
EXTERN(_runtime_mtvt);
PROVIDE(oer_exception_handler = oer_default_exception);
PROVIDE(__oer_interrupt_h0_l1 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l2 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l3 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l4 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l5 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l6 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l7 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l8 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l9 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l10 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l11 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l12 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l13 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l14 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l15 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l16 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l17 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l18 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l19 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l20 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l21 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l22 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l23 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l24 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l25 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l26 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l27 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l28 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l29 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l30 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l31 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l32 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l33 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l34 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l35 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l36 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l37 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l38 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l39 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l40 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l41 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l42 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l43 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l44 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l45 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l46 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h0_l47 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l1 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l2 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l3 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l4 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l5 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l6 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l7 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l8 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l9 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l10 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l11 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l12 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l13 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l14 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l15 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l16 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l17 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l18 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l19 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l20 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l21 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l22 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l23 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l24 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l25 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l26 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l27 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l28 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l29 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l30 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l31 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l32 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l33 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l34 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l35 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l36 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l37 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l38 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l39 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l40 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l41 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l42 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l43 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l44 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l45 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l46 = oer_unhandled_interrupt);
PROVIDE(__oer_interrupt_h1_l47 = oer_unhandled_interrupt);

INCLUDE "runtime/memory.x"
INCLUDE "runtime/sections.x"
