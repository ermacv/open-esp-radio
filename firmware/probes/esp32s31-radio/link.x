ENTRY(_start)

MEMORY
{
  /* SOURCE: probe-link.x derives these symbols from the chip LAYOUT. */
  RTC_FAST (rw) : ORIGIN = PROBE_RETAINED_ORIGIN, LENGTH = PROBE_RETAINED_LENGTH
  TRACE (rwx) : ORIGIN = 0x10000000, LENGTH = 2M
  /* SOURCE: chips/esp32s31/layout/linker/dma-window.x requires nonzero
     low DMA address bits. Leave the null NEXT encoding outside probe storage. */
  DMA (rw)    : ORIGIN = 0x2f000010, LENGTH = 384K - 16
  IRAM (rx)   : ORIGIN = 0x2f060000, LENGTH = 0x1afc0
}

SECTIONS
{
  .rtc_fast.persistent (NOLOAD) : ALIGN(4)
  {
    *(.rtc_fast.persistent .rtc_fast.persistent.*)
  } > RTC_FAST

  .blobray.probes : { KEEP(*(.blobray.probes)) } > TRACE

  .rwtext : ALIGN(4)
  {
    *(.rwtext .rwtext.*)
  } > IRAM

  .text : ALIGN(4)
  {
    /* ENTRY(_start) retains the verifier entry; the unused staged entry
       is not a root of this image. */
    *(.text .text.*)
  } > TRACE

  .rodata : ALIGN(4)
  {
    *(.rodata .rodata.*)
  } > TRACE

  .data : ALIGN(4)
  {
    *(.sdata .sdata.*)
    *(.data .data.*)
  } > TRACE

  /* Production RX APIs reject addresses outside internal DMA SRAM. Keep the
     validation arena in the same physical window instead of weakening that
     invariant for a probe image. */
  .dma_bss (NOLOAD) : ALIGN(16)
  {
    *(.dma.bss .dma.bss.*)
  } > DMA

  .bss (NOLOAD) : ALIGN(4)
  {
    *(.sbss .sbss.*)
    *(.bss .bss.*)
    *(.critical.bss .critical.bss.*)
    *(COMMON)
  } > TRACE

  /DISCARD/ :
  {
    *(.eh_frame .eh_frame.*)
  }
}
