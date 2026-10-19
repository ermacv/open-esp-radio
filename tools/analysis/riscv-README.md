# Shared RV32 layer

The `oer-riscv-*` packages are the RV32 analysis layer that two tools share:
[Blobray](cli/README.md), which researches and compares vendor code,
and the stack analyzer [`oer-analysis-riscv-stack`](riscv-stack/README.md), which bounds the
stacks of our firmware images. Both read ELF through [`oer-analysis-elf`](elf/README.md),
whose `rv32` table is the one classification of RV32 relocations.

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

Every normal or build dependency of a package of this layer or of Blobray on
another of them is allowed by one table, `oer_repo::policy::COMPONENTS`
([repository model](../../README.md)); every package of either application
has a row, and this table and Blobray's state the same. Dependencies on other applications' shared libraries, such as
Blobray's linker adapters on `oer-os-process`, follow the host-application rules.

| Package | May depend on |
| --- | --- |
| `oer-analysis-elf`, `oer-analysis-riscv-model`, `oer-analysis-riscv-decode` | none |
| `oer-analysis-riscv-lift` | `oer-analysis-elf`, `oer-analysis-riscv-decode`, `oer-analysis-riscv-model` |
| `oer-analysis-riscv-analysis` | `oer-analysis-riscv-model` |
| `oer-analysis-riscv-program` | `oer-analysis-elf`, `oer-analysis-riscv-model` |
| `oer-analysis-riscv-stack` | `oer-analysis-elf`, `oer-analysis-riscv-analysis`, `oer-analysis-riscv-decode`, `oer-analysis-riscv-lift`, `oer-analysis-riscv-model`, `oer-analysis-riscv-program` |

The rows of the `blobray-*` crates are in
[Blobray's architecture](blobray-docs/design/architecture.md#crates-and-modules).

Changing an edge changes the table, this page and Blobray's architecture in
the same change.

## Format libraries

A third-party library that reads a format has one owner, and no other
package may take it as a normal or build dependency
(`oer_repo::policy::EXTERNAL_OWNERS`, enforced by the same policy): `object`,
`gimli` and `addr2line` belong to `oer-analysis-elf`, `rv-asm` to `oer-analysis-riscv-decode`.
`object` has one more owner, `oer-analysis-blobray-artifacts`, Blobray's streaming
reader: it reads archive members and sections metered under Blobray's work
budget, keeps the artifact identities its evidence names, and writes ELF
objects, none of which the whole-file view of `oer-analysis-elf` does.
An owner exposes only its own types, so the library's types cannot leak
through another package's interface. Tests may use them for fixtures.
