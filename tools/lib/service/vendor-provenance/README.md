# oer-vendor-provenance

Vendor function provenance. A recovered fact names the vendor function it
came from in a `SOURCE(<chip>):` block, and the citation is registered with
that function's fingerprint in `verification/<chip>/facts/provenance.toml`
(see the [vendor evidence rules](../../verification/README.md)).

`fingerprint` is the one fingerprint of vendor RV32 functions. It reads each
defined function of an archive's members, or of one ELF, through
[`oer-elf`](../elf/README.md) and clears exactly the bits each relocation
patches with that crate's RV32 relocation table. A `Function` carries:

- `code`: the masked bytes and the name-free relocation shape (types, and
  offsets of targets inside the function's own section);
- `named`: `code` plus the names of external targets, so a function whose
  obfuscated callees were renamed keeps its `code` fingerprint;
- `tokens`: one per instruction (RISC-V length encoding), compared by
  `similarity_ppm` (longest common subsequence) and `shingles`;
- `calls`: the named targets of its `call` and `jal` relocations.

[`oer-symbol-lineage`](../symbol-lineage/README.md) uses it too.

The other modules are the provenance check itself; `cargo verification check
provenance`, `cargo verification provenance` and `cargo verification diff`
([verification](../../verification/README.md)) only parse arguments and call
them:

- `citation`: the one recogniser and parser of the `SOURCE` grammar
  (`marker`, `is_marker_line`, `blocks_in`, chip attribution);
- `registry`: the chip's registry (`verification/<chip>/facts/provenance.toml`),
  the survey of every citation — production `SOURCE` blocks, the register
  model's descriptions (read through `oer_register_model::descriptions`),
  verification decisions, ROM summaries and vendor documents — and `check`,
  `violations`, `registered` and `update` (`show` is a callback: the
  library never runs the vendor scenarios);
- `diff`: function-by-function classification of two archive revisions. A test pins one function's fingerprint and `oer-elf` pins the
relocation table: changing either changes registered fingerprints, which are
then regenerated with `cargo verification provenance --chip <chip> --rebuild`
after `cargo verification check provenance` passed with the previous fingerprints.
