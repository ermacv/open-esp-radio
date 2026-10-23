ENTRY(_start)
SECTIONS {
  .ram 0x2f000000 : { *(.ram) }
  .text 0x50000000 : { *(.text .text.*) }
}
