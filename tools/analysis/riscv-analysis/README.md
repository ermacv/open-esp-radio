# Bounded function control-flow and value analysis

Owns a bounded, iterative local CFG, typed output and independent coverage
for section-relative objects and image-addressed functions. It
receives borrowed captured code, structural relocation records, an injected ISA
port, working capacity, control and a result sink. It has no filesystem
authority. Edges to callees never schedule analysis of another function.

The private `values` module owns finite-lattice register states and a bounded
fixed-point queue. It emits values and accesses only after convergence, through
the same borrowed sink as the CFG. Input-dependent allocations share the caller's
working capacity, and all repeated visits share its work/deadline budget.
See [value semantics and limits](../cli/reference/analysis/README.md#values-and-memory-effects).

An optional borrowed `ImageMemory` port supplies immutable load bytes: loaded
sections neither writable nor executable, and writable ones a caller's
read-only window covers whole ([static RV32 images](../riscv-program/README.md));
other writable, code and unmapped memory stays unknown.
Image PC-relative values use virtual addresses, and saved transfer records expose
resolved and unresolved calls/outgoing jumps without expanding the local CFG.
The one exception is `FunctionInput::jumps`: indirect jumps whose targets the
caller knows from the build's facts (`KnownJump`, such as the entries of a jump
table that the link's relocations name), which the CFG follows as jumps.
`research_following_tables` is the one driver that finds them: a
`JumpTableSource` proves, from a pass's records and the build's facts, the
tables of the dispatches the pass locates; the function is analyzed again
following them while each pass proves again every table it followed and
more, and a pass that loses or changes one reports the function from a pass
that follows only the caller's own jumps. `RecognizedTables` is the source for compiler `switch` dispatches. The
value analysis records each jump through `load4[base + (index << 2)]` as a
`Dispatch` record when the load, addition and scaling lie in the jump's own
block and the base the addition reads is a constant, image address or object
symbol: a base the block or a loop's preheader forms holds on every pass even
when the index merges to an unknown value; `jump_tables` reads those records, the
bounds check guarding the dispatch block where its index is known, and asks
the model's `TableEntries` port what the table at `base` holds; every entry
must land inside the function. `oer-analysis-riscv-program` implements the port for
relocatable objects (`ObjectTables`, which needs a bound) and static images
(`ImageTables`: an `--emit-relocs` image's labelled `.LJTI*` table needs
none); Blobray uses both. The stack analyzer's source reads an
`--emit-relocs` image's relocated words, `.LJTI*` labels and sized read-only
objects.
No second analyzer or machine executor is selected for linked code.

The same value solver can emit a flat expression DAG, entry-register values,
symbolic loads, conditions and returns. Record and payload reservations live with the DAG;
loops use the bounded monotone state queue. Explicit ABI context controls only
integer call preservation. ELF ABI flags never select that assumption.
An operation-owned, capacity-admitted hash index interns full `(site, expression)`
keys with stable record IDs and provenance. Hash collisions use full equality;
lookup, replacement-index construction and payload ownership share the analysis
budget. Failed admission or cancellation leaves retained IDs usable.

`PreparedReferences` retains one normalized reference table per prepared section. Sorted offset, symbol and physical-pair indexes replace repeated whole-table scans while preserving nonadjacent HI/LO ambiguity and exact physical identities.

`value_sets` interns canonical sets of at most eight exact leaves in admitted
operation-local vectors and a bounded-load lookup index. It does not enlarge each
register state with an inline array. Joins and Cartesian arithmetic widen explicitly
on a ninth result; stored `alternative-limit` gaps distinguish this loss from a
resolved singleton. Public alternatives are nonrecursive and validated on decode.

The optional `ResearchOptions::symbolic_spills` profile reuses those same bounded
entry-stack words for captured expression values, with exact width/overlap
checks and existing join, alias-write and call invalidation. The default profile
is unchanged. `writes::function_writes` describes argument/field-snapshot-to-store
events over a canonical `FunctionIndex`; `writes::call_writes` substitutes the
call's actual integer ABI inputs and unions its callee effects. These summaries
retain conditional writes and unknown effects; they do not forward mutable
memory contents or establish subsequent reads. See the
[real w1 evidence and limits](../riscv-stack/validation/argument-writes/README.md).
