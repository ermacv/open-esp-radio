# DWARF CU address lookup

The pinned Rust toolchain's fat LTO puts functions from two CUs in `.ram`.
Each CU has a single range. LLVM registers their `DW_AT_low_pc` labels
again when it finalizes the CUs, after the function labels. Its
`.debug_aranges` builder treats that list as address ordered and emits a
backward span, encoded as a very large unsigned length. The third CU's
`leaf` lies inside that spurious range, although its own CU and line
table describe it correctly.

The compiler-driven test links the same objects with and without the
staged images' `dwarf.x`. It checks the malformed accelerator in the
ordinary link, its absence in the staged link, source and inline frames
through the shared `Symbolizer`, and identical allocated sections. Both
RV32 targets use the toolchain pinned by the workspace.
