/* Physical address spaces used by the Flash-resident bootstrap image.
   Addresses and sizes are the chip layout's symbols
   (`oer-espressif-staged-layout` `build`). */
MEMORY
{
    /* The staged chips have unified instruction/data SRAM.  ESP-IDF uses the
       same region for iram_text_seg and dram_seg, so RAM-resident code
       consumes only its actual section size instead of a fixed IRAM
       reservation.

       The SRAM end is ESP-IDF's SRAM_SEG_END; the ROM boot stack side lies
       above it. The second-stage loader runs from SECOND_STAGE_LOADER_START
       (at or below the SRAM end) while it loads this image (see sections.x). */
    SRAM   (RWX) : ORIGIN = SRAM_ORIGIN, LENGTH = SRAM_LENGTH

    /* The unified flash-mapped instruction/data window of the bootstrap. */
    ROTEXT  (RX) : ORIGIN = BOOTSTRAP_FLASH_TEXT_ORIGIN, LENGTH = BOOTSTRAP_FLASH_TEXT_LENGTH

    /* Cached external RAM aperture of the fitted PSRAM. Explicit PSRAM
       sections occupy a prefix; the remainder stays available to the
       application as ordinary external memory. */
    PSRAM   (RWX) : ORIGIN = PSRAM_ORIGIN, LENGTH = PSRAM_LENGTH
}

REGION_ALIAS("RODATA", ROTEXT);
REGION_ALIAS("RWTEXT", SRAM);
REGION_ALIAS("RWDATA", SRAM);
REGION_ALIAS("RTC_FAST_RWTEXT", SRAM);
REGION_ALIAS("RTC_FAST_RWDATA", SRAM);
