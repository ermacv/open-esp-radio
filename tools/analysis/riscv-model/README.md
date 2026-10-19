# RV32 program model

`oer-analysis-riscv-model` owns the identities, errors, run control, working memory
and the ISA-neutral function decoding, lifting and record-navigation contracts that the
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

`SemanticOp::reads_register` and `destination_register` in
[semantics](src/semantics.rs) are the shared queries for described integer
register effects. Integer operands and memory bases/sources count as reads;
`Unsupported` conservatively may read every register. Integer, upper, link,
opaque and memory operations retain their described destination, including
`x0`; no zero-register filtering is implicit. `Fence`, `None` and
`Unsupported` describe no destination.

These queries do not describe every machine-instruction effect: branches
can lift to `None`, indirect jumps to `Link` without their base, and CSR/FP
forms can omit integer inputs. A false read query is not proof of no read,
and no described destination is not proof of no write. Consumers must keep
their instruction-class, control-flow and unknown-effect policies. In the
stack analyzer, task-spawn and typed-address provenance retain their base
instruction gates and flow checks; alias tracking still filters `x0` and
resets at non-base instructions or control transfers. Reaching producers
accept a broader instruction set but explicitly abandon the proof on
`Unsupported` before querying the destination.

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

[Record navigation](src/navigation.rs) `FunctionIndex` borrows one function's
records and admits expression, symbol-reference, call-input and transfer indexes
in the operation's existing `WorkingMemory`. Exact callsite offsets and original
record positions are retained; absent facts stay absent. Construction, indexed
expression/reference/callsite queries and memory-access visits charge the same
`RunControl`. Blobray's canonical evaluators retain a borrowed constant-time
operand primitive, `value_expression`, within their own metered traversals.

`RecordPolicy::Canonical` requires sequential expression IDs in definition order
and earlier Integer/Load operands, rejects duplicate instruction/input/transfer
offsets and inconsistent symbol targets, and permits identical references.
`RecordPolicy::Dictionary` accepts sparse IDs, forward references and cycles and
selects the last source definition for each expression, symbol and input offset.
Neither mode requires complete instruction streams, symbol names, transfer states
or full register arrays. Consumers retain cycle limits, field interpretation,
argument selection and callee selection; the index makes no completeness claim.
