/* Physical address spaces used by the independently linked runtime image.
   Addresses and sizes are the chip layout's symbols
   (`oer-espressif-staged-layout` `build`). */
MEMORY
{
  /* All application-owned internal SRAM is one arena. Section placement is
     determined only by explicitly SRAM-qualified ISR, hot, critical and DMA
     inputs. Linker assertions retain only a bounded transient bootstrap
     handoff margin. */
  INTERNAL_LOW (RWX) : ORIGIN = SRAM_ORIGIN, LENGTH = SRAM_LENGTH
  /* Memory no reset entry initializes: the chip's RTC fast or LP RAM. */
  RTC_FAST (RWX) : ORIGIN = RETAINED_ORIGIN, LENGTH = RETAINED_LENGTH

  /* Stage two after the bootstrap's PSRAM probe page: the relocated
     header, code, read-only and initialized data, then task stacks and BSS. */
  PSRAM_RUNTIME (RWX) : ORIGIN = RUNTIME_PSRAM_ORIGIN, LENGTH = RUNTIME_PSRAM_LENGTH
}
