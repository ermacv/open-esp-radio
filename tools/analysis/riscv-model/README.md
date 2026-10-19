# RV32 program model

`oer-analysis-riscv-model` owns the identities, errors, run control, working memory
and the ISA-neutral function decoding and lifting contracts that the
[analysis](../riscv-analysis/README.md), the [RV32 lifting](../riscv-lift/README.md) and
Blobray share. It has no internal dependency, no filesystem access and no ISA.

[Identity types](src/lib.rs) retain content (`ArtifactId`), object and symbol
identities, and the shared `Error`, `ErrorCode` and `Result`.

[Resource ports](src/resources.rs) define `RunControl` with its run position
and phase, work policy 1 and the default limits (`DEFAULT_WORK_UNITS`,
`DEFAULT_TIMEOUT_MS`). The application supplies the control; domain values
enforce nothing.

[Memory contracts](src/memory.rs) provide `WorkingMemory`, RAII reservations,
fallible `ScratchBytes` and `AdmittedVec`, which admits overlapping buffers
before growth. A reservation can `grow`; a [`Ledger`](src/ledger.rs) admits
the growth of ordinary vectors, maps, sets and strings through one growing
reservation, for structures that outlive their working memory: the caller
holds the ledger while the structure counts against the operation. Borrowed scratch cannot outlive its authority, and live
reservations cannot exceed its capacity. These values account requested
capacity, not resident process pages. `MemoryFailure` carries a rejected
reservation with the phase, input and member of the run position, without
requiring memory from the exhausted pool. [`RecordBuffer`](src/record_memory.rs)
owns function records and their admitted variable capacities.

[Function values](src/function.rs) define physical symbol selectors, extents,
address spaces, per-obligation coverage and the `FunctionDecoder` /
`FunctionSemantics` ports. Typed operations and abstract values describe local
effects without granting access to machine state. `ImageMemory` is a borrowed
immutable-byte port, not a loader or mutable execution bus. Dynamic execution,
inferred function extents and cross-archive definition selection are not
implicit in these values. Calls and possible continuations remain distinct
graph observations.

[Image values](src/image.rs) record the ELF floating-point calling
convention (`RiscvAbi`) separately from the RV32 integer-analysis target,
load segments (`ImageSegment`), borrowed executable sections with their
validated data intervals (`ExecutableSectionView`) and the windows a caller
asserts read-only at run time whatever an image's section flags say
(`ReadOnlyWindow`). The
[byte-source port](src/source.rs) `ByteSource` gives stable positional,
bounded reads of captured content; `hash_source` identifies it.

The [table-entries port](src/tables.rs) `TableEntries` answers what a jump
table holds, given its base (an object's symbol or an image address) and,
when a bounds check states it, its entry count: the entries in index order,
admitted in the caller's `WorkingMemory` (`TableWords`) and charged to its
`RunControl`. A build's facts implement it, as `ImageMemory` answers
constants; `NoTables` is a build whose facts hold no table. The value
analysis locates tables for it with `FunctionRecord::Dispatch`: a jump
through `load4[base + (index << scale)]` whose load, addition and scaling
its own block computes, from a constant, image-address or symbol base.
