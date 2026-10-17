/* Runtime image sections and their relocation/load contracts. */
SECTIONS
{
  /* esp-riscv-rt keeps its reset initializer in the same LTO object as the
     interrupt vectors used by this independently entered runtime. Define the
     chip's RTC-fast (LP RAM) zero/persistent ranges even though runtime
     handoff bypasses the normal reset entry. RTC-fast executable or initialized
     data would require a separate bootstrap copy contract and is rejected. */
  .rtc_fast.unsupported ORIGIN(RTC_FAST) (NOLOAD) :
  {
    *(.rtc_fast.literal .rtc_fast.literal.*);
    *(.rtc_fast.text .rtc_fast.text.*);
    *(.rtc_fast.data .rtc_fast.data.*);
  } > RTC_FAST

  /* `.noinit` promises memory that survives a reset, but stage two keeps
     reset-retained state only in `.rtc_fast.persistent`; any other RAM is
     re-initialized on every boot. Reject the promise instead of zeroing it. */
  /* esp-riscv-rt zeroes `.dram2_uninit.bss` at reset; this platform places
     nothing in the region after the loaded image, so it stays empty. */
  .dram2_uninit.unsupported (NOLOAD) :
  {
    _dram2_uninit_bss_start = ABSOLUTE(.);
    *(.dram2_uninit.bss .dram2_uninit.bss.* .dram2_uninit .dram2_uninit.*);
    _dram2_uninit_bss_end = ABSOLUTE(.);
  } > RTC_FAST

  .noinit.unsupported (NOLOAD) :
  {
    *(.noinit .noinit.*);
    *(.uninit .uninit.*);
  } > RTC_FAST

  .rtc_fast.bss (NOLOAD) :
  {
    . = ALIGN(4);
    _rtc_fast_bss_start = ABSOLUTE(.);
    *(.rtc_fast.bss .rtc_fast.bss.*);
    _rtc_fast_bss_end = ABSOLUTE(.);
  } > RTC_FAST

  .rtc_fast.persistent (NOLOAD) :
  {
    . = ALIGN(4);
    _rtc_fast_persistent_start = ABSOLUTE(.);
    *(.rtc_fast.persistent .rtc_fast.persistent.*);
    _rtc_fast_persistent_end = ABSOLUTE(.);
  } > RTC_FAST

  .runtime.header ORIGIN(PSRAM_RUNTIME) : ALIGN(4)
  {
    __runtime_image_start = ABSOLUTE(.);
    LONG(STAGE_TWO_MAGIC);         /* "STG2" little-endian marker */
    LONG(STAGE_TWO_ABI_VERSION);   /* bootstrap/runtime ABI version */
    LONG(__runtime_image_start);    /* required PSRAM load address */
    LONG(_runtime_start);           /* inherited-stack entry point */
    LONG(__runtime_payload_end);    /* initialized payload end VMA */
    /* The PSRAM BSS the bootstrap clears; runtime clears it again. */
    LONG(__runtime_data_bss_start);
    LONG(__runtime_data_bss_end);
    LONG(STAGE_TWO_HEADER_BYTES);  /* header size in bytes */
    LONG(__runtime_text_start);     /* executable range start */
    LONG(__runtime_text_end);       /* executable range end */
    LONG(0);                       /* host packer writes payload CRC-32 */
  } > PSRAM_RUNTIME

  .isr.text ORIGIN(INTERNAL_LOW) :
    AT(ALIGN(LOADADDR(.runtime.header) + SIZEOF(.runtime.header), 64))
  {
    __runtime_isr_start = ABSOLUTE(.);
    KEEP(*(.trap .trap.*));
    KEEP(*(.isr.text .isr.text.*));
    KEEP(*(.flash.critical.text .flash.critical.text.*));
    *(.rwtext .rwtext.*);
    /* Compiler panic entry helpers can run from an interrupt or an
       exception while caches are unavailable. Match their pinned compiler's v0
       Rust input-section names; the platform's own record/reset code marks
       itself .flash.critical.text.panic. Formatting stays in thread code. */
    *(.text.*_R*4core9panicking*);
    *(.text.*_R*4core6option13expect_failed*);
    *(.text.*_R*4core6option13unwrap_failed*);
    *(.text.*_R*4core6result13unwrap_failed*);
    *(.text.*_R*4core5slice5index*fail*);
    *(.text.*_R*4core4cell*panic_already*);
    . = ALIGN(4);
    __runtime_isr_end = ABSOLUTE(.);
  } > INTERNAL_LOW
  __runtime_isr_load_start = LOADADDR(.isr.text);
  __runtime_isr_load_end = LOADADDR(.isr.text) + SIZEOF(.isr.text);

  /* Modules classify optional performance-sensitive code with semantic
     .hot.text.* input sections, which execute from internal SRAM; this linker
     remains unaware of module names and Rust symbol mangling. */
  .hot.text ALIGN(ADDR(.isr.text) + SIZEOF(.isr.text), 64) :
    AT(ALIGN(LOADADDR(.isr.text) + SIZEOF(.isr.text), 64))
  {
    __runtime_hot_text_start = ABSOLUTE(.);
    *(.hot.text .hot.text.*);
    . = ALIGN(4);
    __runtime_hot_text_end = ABSOLUTE(.);
  } > INTERNAL_LOW
  __runtime_hot_text_load_start = LOADADDR(.hot.text);
  __runtime_hot_text_load_end = LOADADDR(.hot.text) + SIZEOF(.hot.text);

  /* Explicit ISR/flash-safe constants and initialized interrupt-owned state.
     Runtime copies this range before interrupts are enabled. */
  .critical.data ALIGN(__runtime_hot_text_end, 64) :
    AT(ALIGN(__runtime_hot_text_load_end, 64))
  {
    __runtime_critical_data_start = ABSOLUTE(.);
    KEEP(*(.isr.rodata .isr.rodata.*));
    KEEP(*(.flash.critical.rodata .flash.critical.rodata.*));
    KEEP(*(.data.critical .data.critical.*));
    KEEP(*(.critical.data .critical.data.*));
    KEEP(*(.flash.critical.data .flash.critical.data.*));
    . = ALIGN(4);
    __runtime_critical_data_end = ABSOLUTE(.);
  } > INTERNAL_LOW
  __runtime_critical_data_load_start = LOADADDR(.critical.data);
  __runtime_critical_data_load_end = LOADADDR(.critical.data) + SIZEOF(.critical.data);

  .critical.bss ALIGN(ADDR(.critical.data) + SIZEOF(.critical.data), 64) (NOLOAD) :
    AT(ALIGN(ADDR(.critical.data) + SIZEOF(.critical.data), 64))
  {
    __runtime_critical_bss_start = ABSOLUTE(.);
    /* Each hart's interrupt stack: the layout's bytes, a second only on a
       two-hart chip. */
    . = ALIGN(16);
    __runtime_cpu0_irq_stack_bottom = ABSOLUTE(.);
    . = . + IRQ_STACK_BYTES;
    __runtime_cpu0_irq_stack_top = ABSOLUTE(.);
    . = ALIGN(16);
    __runtime_cpu1_irq_stack_bottom = ABSOLUTE(.);
    . = . + (HART_COUNT > 1 ? IRQ_STACK_BYTES : 0);
    __runtime_cpu1_irq_stack_top = ABSOLUTE(.);
    KEEP(*(.critical.bss .critical.bss.*));
    KEEP(*(.flash.critical.bss .flash.critical.bss.*));
    . = ALIGN(64);
    __runtime_critical_bss_end = ABSOLUTE(.);
  } > INTERNAL_LOW

  .dma.data ALIGN(ADDR(.critical.bss) + SIZEOF(.critical.bss), 64) :
    AT(ALIGN(__runtime_critical_data_load_end, 64))
  {
    __runtime_dma_data_start = ABSOLUTE(.);
    KEEP(*(.dma.data .dma.data.*));
    . = ALIGN(4);
    __runtime_dma_data_end = ABSOLUTE(.);
  } > INTERNAL_LOW
  __runtime_dma_data_load_start = LOADADDR(.dma.data);
  __runtime_dma_data_load_end = LOADADDR(.dma.data) + SIZEOF(.dma.data);

  .dma.bss ALIGN(ADDR(.dma.data) + SIZEOF(.dma.data), 64) (NOLOAD) :
    AT(ALIGN(ADDR(.dma.data) + SIZEOF(.dma.data), 64))
  {
    __runtime_dma_bss_start = ABSOLUTE(.);
    KEEP(*(.dma.bss .dma.bss.*));
    . = ALIGN(64);
    __runtime_dma_bss_end = ABSOLUTE(.);
  } > INTERNAL_LOW

  /* Put ordinary code directly after the SRAM exception load image. VMA and
     LMA are identical for both Flash-XIP and relocated-PSRAM code, keeping
     objcopy's flat payload contiguous. */
  .text ALIGN(__runtime_dma_data_load_end, 256) : ALIGN(64)
  {
    __runtime_text_start = ABSOLUTE(.);
    KEEP(*(.text._start));
    KEEP(*(.text.runtime_trap));
    KEEP(*(.text.instruction_stream));
    *(.text .text.*);
    *(.init .init.*);
    __runtime_text_end = ABSOLUTE(.);
  } > PSRAM_RUNTIME

  .rodata : ALIGN(16)
  {
    *(.rodata .rodata.*);
    *(.srodata .srodata.*);
    *(.espressif.metadata .espressif.metadata.*);
  } > PSRAM_RUNTIME

  .data ALIGN(ADDR(.rodata) + SIZEOF(.rodata), 16) :
    AT(ALIGN(LOADADDR(.rodata) + SIZEOF(.rodata), 16))
  {
    __runtime_data_start = ABSOLUTE(.);
    __global_pointer$ = . + 0x800;
    *(.sdata .sdata.*);
    *(.rwdata .rwdata.*);
    *(.data .data.*);
    . = ALIGN(16);
    __runtime_data_end = ABSOLUTE(.);
  } > PSRAM_RUNTIME
  __runtime_data_load_start = LOADADDR(.data);
  __runtime_data_load_end = LOADADDR(.data) + SIZEOF(.data);

  /* PSRAM code is exposed through 64 KiB MMU pages. Keep even a small
     runtime backed by one complete page; otherwise the bootstrap can hand
     execution to an address whose final cache/MMU page is only partially
     represented by the relocated payload. Larger radio images keep their
     natural size instead of being rounded up on every build. */
  .runtime.payload_end ALIGN(__runtime_data_load_end, 16) : ALIGN(16)
  {
    . = MAX(., ORIGIN(PSRAM_RUNTIME) + 0x10000 - 1);
    /* Keep one real byte after the final alignment. llvm-objcopy emits flat
       binaries from section contents, so an empty trailing alignment gap
       would otherwise be omitted from the embedded payload. */
    BYTE(0);
    __runtime_payload_end = ABSOLUTE(.);
  } > PSRAM_RUNTIME

  /* Large CPU-only buffers that their owning Rust modules initialize
     explicitly after handoff; NOLOAD, so neither boot stage clears them. */
  .psram.noinit ALIGN(ADDR(.runtime.payload_end) + SIZEOF(.runtime.payload_end), 64) (NOLOAD) :
  {
    __runtime_psram_noinit_start = ABSOLUTE(.);
    KEEP(*(.psram.noinit .psram.noinit.*));
    . = ALIGN(64);
    __runtime_psram_noinit_end = ABSOLUTE(.);
  } > PSRAM_RUNTIME

  /* Thread-mode stacks. Interrupt and trap stacks remain in the critical SRAM
     range above; these bytes are never DMA-visible and need no bootstrap
     initialization. */
  .psram.task_stacks ALIGN(__runtime_psram_noinit_end, 64) (NOLOAD) :
  {
    __runtime_cpu0_task_stack_bottom = ABSOLUTE(.);
    . = . + CPU0_PSRAM_TASK_STACK_BYTES;
    __runtime_cpu0_task_stack_top = ABSOLUTE(.);
    . = ALIGN(64);
  } > PSRAM_RUNTIME

  .bss ALIGN(__runtime_cpu0_task_stack_top, 64) (NOLOAD) :
  {
    __runtime_data_bss_start = ABSOLUTE(.);
    _bss_start = ABSOLUTE(.);
    /* Explicit cached-PSRAM state must be reset on every boot. Keeping it
       inside runtime BSS prevents persistent NOLOAD ownership flags from
       surviving the image-preflight reset. */
    __runtime_explicit_psram_bss_start = ABSOLUTE(.);
    KEEP(*(.psram.bss .psram.bss.*));
    __runtime_explicit_psram_bss_end = ABSOLUTE(.);
    *(.sbss .sbss.*);
    *(.bss .bss.*);
    *(COMMON);
    . = ALIGN(16);
    __runtime_data_bss_end = ABSOLUTE(.);
    _bss_end = ABSOLUTE(.);
  } > PSRAM_RUNTIME

  __runtime_bss_start = __runtime_data_bss_start;
  __runtime_bss_end = __runtime_data_bss_end;
  __runtime_image_end = __runtime_data_bss_end;
  __sbss = __runtime_data_bss_start;
  __ebss = __runtime_data_bss_end;

  /* The CPU0 PSRAM task stack is the ordinary stack range; CLIC and
     exception entries use separate per-hart SRAM stacks. */
  _dram_data_start = SRAM_ORIGIN;
  _stack_end = __runtime_cpu0_task_stack_bottom;
  _stack_end_cpu0 = _stack_end;
  _stack_start = __runtime_cpu0_task_stack_top;
  _stack_start_cpu0 = _stack_start;

  /DISCARD/ :
  {
    *(.eh_frame .eh_frame.*);
    *(.eh_frame_hdr);
  }
}

ASSERT(__runtime_payload_end <= ORIGIN(PSRAM_RUNTIME) + LENGTH(PSRAM_RUNTIME),
       "runtime initialized payload does not fit PSRAM");
ASSERT(SIZEOF(.rtc_fast.unsupported) == 0,
       "runtime RTC-fast code/data requires an explicit bootstrap copy contract");
ASSERT(SIZEOF(.noinit.unsupported) == 0,
       "stage two retains state across a reset only in .rtc_fast.persistent");
/* Stage two's entry does not clear RTC-fast BSS (the bootloader and ROM may
   leave anything there); zero-initialized RTC-fast state needs that clearing
   first. */
ASSERT(SIZEOF(.rtc_fast.bss) == 0,
       "runtime entry does not zero .rtc_fast.bss");
/* ESP-IDF keeps the tail of the retained memory as lp_reserved_seg
 * (RESERVE_RTC_MEM); the chip layout states its size for the bootloader
 * this platform boots through. Runtime RTC-fast data grows up from ORIGIN
 * and must stay below it. */
ASSERT(_rtc_fast_persistent_end <= ORIGIN(RTC_FAST) + LENGTH(RTC_FAST) - RETAINED_RESERVED_TAIL,
       "runtime RTC-fast data reaches ESP-IDF's reserved LP RAM tail");
ASSERT(__runtime_psram_noinit_end <= ORIGIN(PSRAM_RUNTIME) + LENGTH(PSRAM_RUNTIME),
       "runtime explicit no-init storage does not fit");
ASSERT(__runtime_cpu0_task_stack_top <= ORIGIN(PSRAM_RUNTIME) + LENGTH(PSRAM_RUNTIME),
       "runtime task stacks do not fit");
ASSERT(__runtime_data_bss_end <= ORIGIN(PSRAM_RUNTIME) + LENGTH(PSRAM_RUNTIME),
       "runtime data/BSS does not fit PSRAM");
ASSERT(__runtime_data_load_start >= ORIGIN(PSRAM_RUNTIME) &&
       __runtime_data_load_end <= ORIGIN(PSRAM_RUNTIME) + LENGTH(PSRAM_RUNTIME),
       "runtime initialized data load image is outside payload");
ASSERT(__runtime_isr_end <= ORIGIN(INTERNAL_LOW) + LENGTH(INTERNAL_LOW),
       "runtime interrupt code does not fit internal SRAM");
ASSERT(__runtime_hot_text_end <= ORIGIN(INTERNAL_LOW) + LENGTH(INTERNAL_LOW),
       "runtime hot code does not fit internal SRAM");
ASSERT(__runtime_critical_data_end <= ORIGIN(INTERNAL_LOW) + LENGTH(INTERNAL_LOW),
       "runtime initialized interrupt state does not fit internal SRAM");
ASSERT(__runtime_critical_bss_end <= ORIGIN(INTERNAL_LOW) + LENGTH(INTERNAL_LOW),
       "runtime critical state does not fit low internal SRAM");
ASSERT(__runtime_dma_data_end <= ORIGIN(INTERNAL_LOW) + LENGTH(INTERNAL_LOW),
       "runtime initialized DMA state does not fit internal SRAM");
ASSERT(__runtime_dma_bss_end <= ORIGIN(INTERNAL_LOW) + LENGTH(INTERNAL_LOW),
       "runtime DMA storage does not fit internal SRAM");
/* The bootstrap's largest post-LTO frame is below 0.5 KiB. Keep a bounded
   8-KiB transient handoff margin until `_runtime_stack_bootstrap` switches
   CPU0 to PSRAM; unlike the former 64-KiB hole this is not a task stack and
   cannot silently grow. */
ASSERT(__runtime_dma_bss_end <=
       ORIGIN(INTERNAL_LOW) + LENGTH(INTERNAL_LOW) - 0x2000,
       "runtime SRAM owners overlap the 8-KiB bootstrap handoff margin");
ASSERT((__runtime_isr_load_start & 3) == 0 &&
       ((__runtime_isr_end - __runtime_isr_start) & 3) == 0,
       "runtime interrupt copy must be word aligned");
ASSERT((__runtime_hot_text_load_start & 3) == 0 &&
       ((__runtime_hot_text_end - __runtime_hot_text_start) & 3) == 0,
       "runtime hot-code copy must be word aligned");
ASSERT((__runtime_dma_bss_start & 3) == 0 &&
       ((__runtime_dma_bss_end - __runtime_dma_bss_start) & 3) == 0,
       "runtime DMA BSS must be word aligned");
ASSERT((__runtime_critical_bss_start & 3) == 0 &&
       ((__runtime_critical_bss_end - __runtime_critical_bss_start) & 3) == 0,
       "runtime critical BSS must be word aligned");
ASSERT((__runtime_critical_data_load_start & 3) == 0 &&
       ((__runtime_critical_data_end - __runtime_critical_data_start) & 3) == 0,
       "runtime initialized interrupt state copy must be word aligned");
ASSERT((__runtime_dma_data_load_start & 3) == 0 &&
       ((__runtime_dma_data_end - __runtime_dma_data_start) & 3) == 0,
       "runtime initialized DMA state copy must be word aligned");
ASSERT(__runtime_cpu0_irq_stack_top - __runtime_cpu0_irq_stack_bottom == IRQ_STACK_BYTES &&
       __runtime_cpu1_irq_stack_top - __runtime_cpu1_irq_stack_bottom ==
           (HART_COUNT > 1 ? IRQ_STACK_BYTES : 0),
       "runtime requires one exact SRAM IRQ stack per hart");
ASSERT(__runtime_cpu0_task_stack_top - __runtime_cpu0_task_stack_bottom == CPU0_PSRAM_TASK_STACK_BYTES,
       "runtime requires one exact CPU0 PSRAM task stack");
ASSERT(SIZEOF(.runtime.header) == STAGE_TWO_HEADER_BYTES,
       "stage-two header does not match the platform layout");
ASSERT(_runtime_start >= __runtime_text_start && _runtime_start < __runtime_text_end,
       "runtime entry point is outside executable text");
ASSERT(SIZEOF(.dram2_uninit.unsupported) == 0,
       "this platform places nothing in .dram2_uninit");
