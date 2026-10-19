ENTRY(_start)

MEMORY
{
  TRACE (rwx) : ORIGIN = 0x10000000, LENGTH = 2M
  DMA (rw)    : ORIGIN = 0x2f000000, LENGTH = 384K
  IRAM (rx)   : ORIGIN = 0x2f060000, LENGTH = 0x1afc0
}

SECTIONS
{
  .blobray.probes : { KEEP(*(.blobray.probes)) } > TRACE

  .rwtext : ALIGN(4)
  {
    *(.rwtext .rwtext.*)
  } > IRAM

  .text : ALIGN(4)
  {
    KEEP(*(.text._start))
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
    *(COMMON)
  } > TRACE

  /DISCARD/ :
  {
    *(.eh_frame .eh_frame.*)
  }
}
