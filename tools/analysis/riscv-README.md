# Shared RV32 layer

The `oer-analysis-riscv-*` packages are the RV32 analysis layer that two tools share:
[Blobray](cli/README.md), which researches and compares vendor code,
and the stack analyzer [`oer-analysis-riscv-stack`](riscv-stack/README.md), which bounds the
stacks of our firmware images. Both read ELF through [`oer-analysis-elf`](elf/README.md),
whose `rv32` table is the one classification of RV32 relocations.

The [question-owner map](blobray-docs/design/architecture.md#question-owners)
records which module owns each analysis question and the current overlapping
paths. New analyses extend that owner; duplicate implementations need a reason
in the map.

Limits distinguish input validation, resource budgets and precision bounds.
Validation rejects invalid input; precision bounds leave explicit unknown
evidence. Work budgets belong to a function or query: local exhaustion degrades
that result with a typed reason and a report count. The operation-wide work,
deadline, cancellation and admitted-memory limits are safeguards whose original
errors terminate the operation. Summary catalogs retain proved, no-proof and
local-budget outcomes; register reports count these functions and the reasons for
unavailable ROM summaries and incomplete signature audits. See the
[limit contract](blobray-docs/design/architecture.md#question-owners).

| Package | Owns |
| --- | --- |
| [`oer-analysis-riscv-model`](riscv-model/README.md) | Identities, errors, run control, working memory and the function-record vocabulary every layer shares |
| [`oer-analysis-riscv-decode`](riscv-decode/README.md) | Instruction bytes to typed instructions for a selected extension set |
| [`oer-analysis-riscv-lift`](riscv-lift/README.md) | Function decoding, relocation interpretation, instruction semantics and placement-independent listings |
| [`oer-analysis-riscv-analysis`](riscv-analysis/README.md) | The bounded control-flow and value analysis of one function |
| [`oer-analysis-riscv-program`](riscv-program/README.md) | The view of one static RV32 image: segments, image memory, code symbols and executable sections |
| [`oer-analysis-riscv-stack`](riscv-stack/README.md) | Worst-case stack bounds of a static image; a consumer of the layer, not part of Blobray |

The layer is Blobray's ISA layer: Blobray's application depends on the
ISA-neutral model, the function analysis and the static image view, and its
command line selects the concrete decoder from `oer-analysis-riscv-lift`. RV32 facts,
such as relocation types and register numbers, belong to this layer, never to
Blobray's application.

## Dependency table

The workspace manifests declare these internal dependencies. Blobray's
component boundaries are described in its
[architecture](blobray-docs/design/architecture.md#crates-and-modules).
Its linker adapters also use the host process library.

| Package | May depend on |
| --- | --- |
| `oer-analysis-elf`, `oer-analysis-riscv-model`, `oer-analysis-riscv-decode` | none |
| `oer-analysis-riscv-lift` | `oer-analysis-elf`, `oer-analysis-riscv-decode`, `oer-analysis-riscv-model` |
| `oer-analysis-riscv-analysis` | `oer-analysis-riscv-model` |
| `oer-analysis-riscv-program` | `oer-analysis-elf`, `oer-analysis-riscv-model` |
| `oer-analysis-riscv-stack` | `oer-analysis-elf`, `oer-analysis-riscv-analysis`, `oer-analysis-riscv-decode`, `oer-analysis-riscv-lift`, `oer-analysis-riscv-model`, `oer-analysis-riscv-program` |

The rows of the `blobray-*` crates are in
[Blobray's architecture](blobray-docs/design/architecture.md#crates-and-modules).

The table describes repository dependencies, excluding test-only fixtures.

## Format libraries

A third-party library that reads a format has one owner, and no other
package takes it as a normal or build dependency in this layer: `object`,
`gimli` and `addr2line` belong to `oer-analysis-elf`, `rv-asm` to `oer-analysis-riscv-decode`.
`object` has one more owner, `oer-analysis-blobray-artifacts`, Blobray's streaming
reader: it reads archive members and sections metered under Blobray's work
budget, keeps the artifact identities its evidence names, and writes ELF
objects, none of which the whole-file view of `oer-analysis-elf` does.
An owner exposes only its own types, so the library's types cannot leak
through another package's interface. Tests may use them for fixtures.
