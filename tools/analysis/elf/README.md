# oer-analysis-elf

The one view of ELF files and static archives that every host tool reads
through; nothing else parses `llvm-nm` or `llvm-objdump` output or walks
`object` for symbols, sections or relocations.

- `Elf::parse` (any ELF), `Elf::executable` (a static little-endian RV32
  image), `objects(bytes)` (each ELF member of an archive, or the one file)
  and `is_binary`.
- `classify` classifies raw symbol types and section indices (including
  `SHN_XINDEX` with its extended index), section types and flags, and object
  types. Its header-only `is_executable_image` recognizes little-endian
  executables without another inventory pass. Consumers of captured ELF
  facts share these classifications without depending on `object`.
- Symbols: `symbols()`, `symbol(name)`, `address(name)`, `addresses()`,
  `demangle(name)`. `functions()` merges every code symbol at one address
  into one `Function` with all its names (identical-code-folded aliases),
  and extends an unsized entry to the next code symbol.
- Sections: `sections()`, `section(index)`, `section_by_name`, with their
  allocation, write, execute, TLS and `NOBITS` flags, header type
  (`SectionKind`), the section a `REL`/`RELA` section relocates
  (`relocates`), file offset and bytes (a section whose range leaves the
  file reads as empty: check `file_offset + size` before trusting it);
  `symbol_table_section()` and `dynamic_symbols()`. `raw_symbols()` lists
  every entry of both tables with its name as raw bytes, `None` when it
  cannot be read, where `symbols()` skips such an entry.
- Program headers: `program_headers()`, each with its type
  (`ProgramHeaderKind`: load, dynamic, interpreter, TLS), addresses, file and
  memory sizes and permissions; `header_flags()` is the raw `e_flags`, which
  `rv32::header_flags` decodes into the floating-point ABI and RV32E.
- No `object` type appears in the public interface: callers see only these
  views and this crate's `Error`. `object`, `gimli` and `addr2line` are this
  crate's to depend on; `oer-analysis-blobray-artifacts`, Blobray's metered streaming
  reader and object writer, is the one other owner of `object`
  ([format libraries](../riscv-README.md#format-libraries)).
- Relocations: `relocations(section)`, each with an explicit or implicit
  (`SHT_REL`, in the patched word) addend, and `target_address`. `rv32` is the
  one RV32 relocation table: each type's patched `Field` and its target's
  `Role` (call, jump, branch, absolute or PC-relative address halves, data
  word, hint, other), and `mask`, which clears exactly the patched bits.
  An unknown type masks its whole word. The vendor fingerprints, the stack
  analysis, the lifter and the vendor report all classify relocations
  through it; a test pins every row.
- `code(member)`: the defined functions of a relocatable object with their
  bytes and relocation `Site`s, each target resolved to an offset in the
  function's own section (`Reference::Local`), a named symbol, an anonymous
  section, or nothing. The vendor fingerprints are computed over it
  ([`oer-vendor-provenance`](../../vendor/provenance/README.md)).
- `dwarf::Symbolizer::frames(address)`: the inline chain at an address with
  each frame's demangled function, file and line. `Symbolizer::new` copies
  each debug section; `Symbolizer::shared` reads them as ranges of the ELF's
  shared bytes. `dwarf::load` gives the debug information itself to
  analyses that read DWARF entries; `dwarf::DebugSections` lends it from
  the ELF's bytes without copies. A compressed debug section is an error:
  `object` is built without its compression feature.

It has no repository path dependency, so Blobray's standalone extraction
takes it by path beside the RV32 crates. Blobray's own artifact inventory
(`tools/analysis/cli/crates/artifacts`) stays separate: it streams member and
section bytes under Blobray's working-memory and cancellation budget and
keeps artifact, member and symbol-table identities for evidence, which a
whole-file view does not.
