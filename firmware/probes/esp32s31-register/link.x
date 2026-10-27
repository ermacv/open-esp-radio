ENTRY(_start)

MEMORY
{
  /* SOURCE: probe-link.x derives these symbols from the chip LAYOUT. */
  RTC_FAST (rw) : ORIGIN = PROBE_RETAINED_ORIGIN, LENGTH = PROBE_RETAINED_LENGTH
  TRACE (rwx) : ORIGIN = 0x10000000, LENGTH = 2M
}

SECTIONS
{
  .rtc_fast.persistent (NOLOAD) : ALIGN(4)
  {
    *(.rtc_fast.persistent .rtc_fast.persistent.*)
  } > RTC_FAST

  .blobray.probes : { KEEP(*(.blobray.probes)) } > TRACE

  .text : ALIGN(4)
  {
    /* ENTRY(_start) retains the verifier entry; the unused staged entry
       is not a root of this image. */
    *(.text .text.*)
  } > TRACE

  .rodata : ALIGN(4) { *(.rodata .rodata.*) } > TRACE
  .data : ALIGN(4) { *(.sdata .sdata.*) *(.data .data.*) } > TRACE
  .bss (NOLOAD) : ALIGN(4)
  {
    *(.sbss .sbss.*)
    *(.bss .bss.*)
    *(.critical.bss .critical.bss.*)
    *(COMMON)
  } > TRACE
  /DISCARD/ : { *(.eh_frame .eh_frame.*) }
}
