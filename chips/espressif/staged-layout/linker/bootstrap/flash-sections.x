/* Flash-mapped metadata, constants and relocation payloads owned by bootstrap. */
SECTIONS {
  .flash.appdesc : ALIGN(4)
  {
      KEEP(*(.flash.appdesc));
      KEEP(*(.flash.appdesc.*));
  } > RODATA

  .flash.rodata : ALIGN(4)
  {
    . = ALIGN(4);

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

  } > RODATA

  /* SOURCE: ESP-IDF e9da155a72624fce88b8ef2cf3cde9aee2e6067f,
     bootloader_support/src/bootloader_utility.c:802-851. The S31 loader
     requires two mapped ESP segments in the shared D/I window, without
     requiring executable flash. Keep the real tuning source disjoint from
     the stage-two payload; the packer merges adjacent ELF sections.
     The encoded-image audit checks the loader contract in chip facts. */
  .flash.tuning.reference ALIGN(MMU_PAGE_BYTES) + IMAGE_SEGMENT_HEADER_BYTES :
  {
    /* Flash::tune_120mhz consumes cold XIP pages. Runtime copying happens
       first, so keep a disjoint page-aligned reference span for the sweep.
       A chip whose bootstrap does not tune Flash keeps it empty. */
    . = ALIGN(4096);
    __flash_tuning_reference_start = ABSOLUTE(.);
    KEEP(*(.flash.tuning.reference));
    __flash_tuning_reference_end = ABSOLUTE(.);

  } > RODATA
}

ASSERT((__flash_tuning_reference_start & 0xfff) == 0 &&
       (__flash_tuning_reference_end - __flash_tuning_reference_start) >=
           FLASH_TUNING_REFERENCE_BYTES,
       "Flash tuning reference must span the layout's aligned reference bytes");
