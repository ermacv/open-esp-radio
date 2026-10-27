# Register outputs and checks

`oer-registers-generate` is the service behind `cargo registers`. It renders
a chip's generated outputs from its register model
(`chips/<chip>/registers/`: the root `chip.toml`, the block types and
layouts it includes, every evidence catalog of `evidence/` and the ownership
policy `policy.toml`; [format](../model/README.md)) and runs the register
checks.

```console
cargo registers validate --chip esp32s31
cargo registers generate --chip esp32s31 --check
```

`validate` loads the model and the policy and checks every rule of both.
`generate` renders, formats and writes:

| Output | What it holds |
| --- | --- |
| `chips/<chip>/pac/src/generated.rs` | The thin PAC (`chip_render::render`): typed blocks, field accessors, value domains, typed pointers, interrupt sources, instances, ownership parts and partitions, the irreducible policy items, memory layouts and the analog-I2C dictionary |
| `chips/<chip>/layout/linker/dma-window.x` | The DMA window and the low address bits its pointers keep (`chip_render::dma_window`), which the images' link checks DMA storage against |

Every output is rendered before any is written; each file is replaced
atomically. `--check` writes nothing and fails if an output is missing or
differs. Host tools read the register model itself
(`oer_registers_model::Chip::load`), each register's address, width and
fields from `ChipModel::registers()`.

## Register checks

`cargo registers check inventory --chip <chip>` checks vendor uses of register
bits against the source model. It calls Blobray's `library::register_accesses`
over every function of every pinned vendor binary, excluding firmware outputs,
inside the chip root's MMIO instance windows, with `--abi riscv-integer
--call-summaries`. Each typed record is folded into compared words as it
arrives, without a child process or a `registers.json` corpus. It requires a
complete `reads: ReadUses`
classification for each integer or floating load, using the shared
`oer-analysis-blobray-domain` types. A `CallArgument` to an exact name in the
checker's [chip-specific cited diagnostic lists](src/checks/inventory/uses/read_policy.rs)
does not count as use. Other sinks, including unresolved calls and behavior
parameters such as `ets_delay_us`, count with both `Exact` and `May` precision;
read use comes only from load dependencies. Diagnostic evidence is
retained, and never hides another use or an unknown observation of the same bit.

Stores count explicit `write-replacement` bits and constant bits that differ
from the inherited known reset. Same-position same-word preservation and
whole-word reset writes are not uses. Unknown reset or stored value remains
unknown unless an explicit replacement mask establishes use.

The check writes `target/register-inventory/<chip>/report.txt` and `report.json`
before failing on used bits without fields. The JSON envelope is schema 1;
`analysis_resources` records the `register-inventory` profile, effective limits,
work and admitted-memory usage, and typed affected events, functions and records.
Counters are merged across disjoint inputs while call summaries still see the
whole corpus. `inventory-enumerated-indices` and `declared-word-runs` remain typed
unaccounted limits. `reserved_violation_bits` are
uses of a source's explicitly reserved bits, with per-source masks;
`absent_violation_bits` are uses of bits without a field or reserved annotation.
Both are errors. A register's `reserved = { mask, sources }` in its block
type names the source of each reserved bit; it never creates PAC fields. `may_only_bits` lists each word's
uses supported only by `May`, excluding any exact read or proven write use.
Read accounting ranks callees by source bits, retaining targets and diagnostic
SOURCE citations, and reports diagnostic-call and diagnostic-only shares.
Each load's source bits are unioned across sites/arguments before indexed
address expansion; callee groups can overlap. Resolved/indexed MMIO and all
observations including unresolved addresses have separate totals. Unknown bits
are listed separately by word, with their reasons and source symbols, and do not create opaque fields
or establish absence. The reports retain input hashes, assumptions, ranges,
partial/blocked/gap counts and causes, and unresolved/indexed address limits.
Access counts and the provenance registry's cited-function priority are retained;
a citation alone is not evidence that any particular bit is used.
Small bounded indices compare every candidate; larger or unbounded progressions
compare declared arrays and runs and retain a coverage limit. The report
retains the summary returned by the same analysis and a `cargo blobray`
command that reproduces its corpus as JSON for manual inspection.
The MMIO corpus does not cover RAM layouts. No model is edited by this check.

## Pinned header function signatures

`cargo registers signatures --chip esp32s31 --idf <checkout>
--sdkconfig <build/config/sdkconfig.h> [--libc-include <picolibc/include>]
[--clang clang] [--check] [--output <ignored-report-directory>]` writes
`chips/<chip>/vendor/facts/idf-function-signatures.toml` and its companion
`idf-function-signatures.provenance.toml`. Use the complete ESP-IDF checkout
at the chip's current pin, with its pinned header submodules initialized.
`--check` repeats generation and requires byte-for-byte equal facts.

The SDK configuration is an explicit input. The checked-in facts use the
generated configuration of `firmware/vendor/esp32s31/ble-scan-reference`,
configured for `IDF_TARGET=esp32s31` with the pinned ESP-IDF and the project's
`sdkconfig.defaults`. CMake configuration produces `config/sdkconfig.h`
without building firmware. Keep its build directory and `SDKCONFIG` under
the worktree's `target/`. The configuration hash is recorded, and another
configuration may expose a different set of declarations. A RISC-V capable
Clang and the SDK's picolibc headers are required.

The ELF facade supplies global name candidates from every pinned binary;
Blobray `call-arguments --call-summaries --abi riscv-integer` selects actual
named call targets. This includes externally linked functions implemented
elsewhere in the corpus. Coverage reports separately identify targets with
no captured executable code bytes; an absolute import symbol alone does
not establish a captured implementation. Partial analysis is reported and
does not prove absence of further calls.

The generator compiles matching open headers as RV32 ILP32 C, follows the
compiler's resolved type graph and queries sizes in that same translation
unit. It publishes four fields per `[[function]]`: `name`, `arguments`,
`result`, `source`. `arguments` counts occupied a0..a7 words, capped at eight:
`(void *, uint64_t)` consumes three. Named 64-bit scalars occupy consecutive
words under the [integer psABI](https://riscv-non-isa.github.io/riscv-elf-psabi-doc/#_integer_calling_convention).
Stack arguments do not allow discarding a register. Results are `void`,
`u32` (pointers or integers of at most 32 bits), or `u64` (64-bit integers).
Aggregates by value, variadic functions, floating types, old-style
declarations and other unsupported types supply no contract. A compiler
error invalidates the whole header context. Conflicting supported contracts
or supported/unsupported declaration variants stop publication.

Each record cites a pinned header path and line, followed by a header hash
comment. The companion provenance records header hashes, SDK configuration
hash and complete binary input hashes without local cache paths. Published
headers must match committed bytes, including the parent commit's gitlink
for a header in a submodule. Facts are Apache-2.0; the generator remains
under the workspace license. It copies no source declarations or code.

The ignored report directory defaults to `target/register-signatures/<chip>`.
The child Blobray uses `--analysis-profile signature-corpus` (256 MiB,
1,000,000,000 work units, 3600 seconds). The schema-1 `coverage.json` retains
its complete `corpus.analysis_resources` without recalculating the child counts.
The directory contains the raw call capture, compiler diagnostics and `coverage.json`:
covered names, targets without captured definitions, unsupported prototype
reasons, failed header contexts, identifiers without qualified declarations
and names absent from all selected headers. Identifier search selects work;
only compiler-qualified declarations create facts. Header coverage does not
establish agreement with binary call behavior: integration with the signature
consumer must report any contract/summary conflict before using a contract.
