/* The ESP image loader places initialized SRAM segments at their linked
   addresses. Reset clears BSS only; noinit and retained memory are preserved. */
SECTIONS {
  .rwtext : ALIGN(4) {
    _rwtext_start = ABSOLUTE(.);
    KEEP(*(.init .init.*));
    _trap_section_origin = ABSOLUTE(.);
    KEEP(*(.trap .trap.*));
    *(.rwtext .rwtext.* .rwtext.literal .rwtext.literal.*);
    /* Cache suspension and flash tuning call across crate boundaries.
       A caller's section attribute does not place its out-of-line callees. */
    *(.text .text.* .literal .literal.*);
    . = ALIGN(4);
    _rwtext_end = ABSOLUTE(.);
  } > SRAM

  .code_bench.iram (NOLOAD) : ALIGN(64) {
    __code_bench_iram_start = ABSOLUTE(.);
    . += __code_bench_flash_end - __code_bench_flash_start;
    __code_bench_iram_end = ABSOLUTE(.);
  } > SRAM

  .data : ALIGN(4) {
    _data_start = ABSOLUTE(.);
    /* SOURCE: RISC-V ELF psABI, global-pointer relaxation: signed 12-bit offsets. */
    __global_pointer$ = . + 0x800;
    *(.sdata .sdata.* .data .data.* .data1);
    *(.critical.data .critical.data.*);
    . = ALIGN(4);
    _data_end = ABSOLUTE(.);
  } > SRAM
  _dram_data_start = _data_start;

  /* Constants, vtables and compiler-generated tables may be read while
     instruction fetch is suspended, including from a failure path. */
  .rodata : ALIGN(4) {
    *(.rodata .rodata.* .srodata .srodata.* .rodata_wlog_*.*);
    . = ALIGN(4);
  } > SRAM

  .data.wifi : ALIGN(4) {
    *(.data_wlog_*.*);
    . = ALIGN(4);
  } > SRAM

  .bss (NOLOAD) : ALIGN(4) {
    _bss_start = ABSOLUTE(.);
    *(.sbss .sbss.* .bss .bss.* COMMON);
    *(.critical.bss .critical.bss.*);
    . = ALIGN(4);
    _bss_end = ABSOLUTE(.);
  } > SRAM

  .noinit (NOLOAD) : ALIGN(4) {
    *(.noinit .noinit.*);
  } > SRAM

  .stack (NOLOAD) : ALIGN(16) {
    _stack_end = ABSOLUTE(.);
    . = ORIGIN(SRAM) + LENGTH(SRAM);
    _stack_start = ABSOLUTE(.);
  } > SRAM

  .rtc_fast.persistent RETAINED_ORIGIN (NOLOAD) : ALIGN(4) {
    *(.rtc_fast.persistent .rtc_fast.persistent.*);
  } > RETAINED

  .psram.unsupported (NOLOAD) : {
    *(.psram.text .psram.text.* .psram.rodata .psram.rodata.*);
    *(.psram.data .psram.data.* .psram.bss .psram.bss.* .psram.noinit .psram.noinit.*);
    *(.dram2_uninit .dram2_uninit.*);
    *(.rtc_fast.bss .rtc_fast.bss.* .rtc_fast.data .rtc_fast.data.*);
    *(.rtc_fast.text .rtc_fast.text.* .rtc_fast.literal .rtc_fast.literal.*);
  } > PSRAM
}

INCLUDE "bootstrap/flash-sections.x"
SECTIONS {
  .eh_frame : ALIGN(4) { KEEP(*(.eh_frame .eh_frame.*)); } > RODATA
  .eh_frame_hdr : ALIGN(4) { *(.eh_frame_hdr); } > RODATA
  .metadata : { *(.esp.metadata .esp.metadata.*); } > RODATA
  /DISCARD/ : { *(.comment .note .note.*); }
}

ASSERT(SIZEOF(.psram.unsupported) == 0,
       "the bootstrap owns no PSRAM sections; stage two owns PSRAM");
ASSERT(ADDR(.data.wifi) + SIZEOF(.data.wifi) <= SECOND_STAGE_LOADER_START,
       "a loaded bootstrap SRAM segment reaches the second-stage loader");
ASSERT(_stack_start > _stack_end, "the bootstrap has no stack space");
ASSERT(SIZEOF(.rtc_fast.persistent) <= LENGTH(RETAINED), "retained memory overflow");
