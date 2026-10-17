/* Flash-mapped metadata, constants and relocation payloads owned by bootstrap. */
SECTIONS {
  .flash.appdesc : ALIGN(4)
  {
      KEEP(*(.flash.appdesc));
      KEEP(*(.flash.appdesc.*));
  } > RODATA

  .rodata_merge : ALIGN(4) {
    . = ALIGN(ALIGNOF(.rodata));
  } > RODATA

  .rodata : ALIGN(4)
  {
    . = ALIGN(4);
    _rodata_start = ABSOLUTE(.);

    /* The code-location benchmark emits one canonical instruction stream.
       It executes in place here and is copied byte-for-byte to IRAM and
       PSRAM, so all three measurements use identical machine code. */
    . = ALIGN(64);
    __code_bench_flash_start = ABSOLUTE(.);
    KEEP(*(.code_bench.source));
    __code_bench_flash_end = ABSOLUTE(.);

    /* The stage-two image is carried as ordinary Flash rodata and copied to
       PSRAM after external-memory initialization. */
    . = ALIGN(64);
    __psram_runtime_payload_flash_start = ABSOLUTE(.);
    KEEP(*(.psram.runtime.payload));
    __psram_runtime_payload_flash_end = ABSOLUTE(.);

    /* Flash::tune_120mhz consumes cold XIP pages. Runtime copying happens
       first, so keep a disjoint page-aligned reference span for the sweep.
       A chip whose bootstrap does not tune Flash keeps it empty. */
    . = ALIGN(4096);
    __flash_tuning_reference_start = ABSOLUTE(.);
    KEEP(*(.flash.tuning.reference));
    __flash_tuning_reference_end = ABSOLUTE(.);

    *(.rodata .rodata.*)
    *(.srodata .srodata.*)

    _rodata_end = ABSOLUTE(.);
  } > RODATA

  .rodata.wifi : ALIGN(4)
  {
    . = ALIGN(4);
    *(.rodata_wlog_*.*)
    . = ALIGN(4);
  } > RODATA
}

ASSERT((__flash_tuning_reference_start & 0xfff) == 0 &&
       (__flash_tuning_reference_end - __flash_tuning_reference_start) >=
           FLASH_TUNING_REFERENCE_BYTES,
       "Flash tuning reference must span the layout's aligned reference bytes");
