/* LLVM 22's fat-LTO aranges builder can append a CU's low_pc after later
   functions in a shared input section, encoding a backward span as an
   enormous unsigned length. Keep the authoritative CU ranges and line
   tables; omit this optional, redundant address-to-CU accelerator.
   Read this script before any script that places .debug_aranges. */
SECTIONS
{
  /DISCARD/ : { *(.debug_aranges) }
}
