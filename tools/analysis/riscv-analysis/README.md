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
value analysis records each jump through `load4[base + (index << 2) + displacement]` as a
`Dispatch` record when the load, addition and scaling lie on the jump's unique
predecessor chain and the base the addition reads is a constant, image address
or object symbol: a base the block or a loop's preheader forms holds on every pass even
when the index merges to an unknown value; `jump_tables` reads those records, the
bounds check guarding the dispatch block where its index is known, and asks
the model's `TableEntries` port what the table holds. An unlabelled table
starts at the RV32 wrapping address `base + displacement`; a sized data
object must start exactly there. A labelled table has two candidate starts:
`base + displacement`, and `base` for a negative displacement divisible by
`1 << scale`, with at most 4096 skipped indices. Exactly one may start a
labelled table; ambiguity proves nothing. The second candidate uses the whole
labelled chain and adds the skipped indices to a known `first_case`, leaving
an unknown case value unknown. Objects and bounds cannot justify this second
candidate. Every entry must land inside the function.
`oer-analysis-riscv-program` implements the port for
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
The local pool holds `8*N+32` expressions for `N` instructions. When full,
new expressions become unknown and existing IDs remain usable. The solver
continues to convergence and marks the function partial with `ExpressionLimit`;
working-memory and caller run-control failures still fail the operation.

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

`function_writes` also accepts already available callee summaries for one level
of transparent tail composition. Canonical facts must prove exactly one
unconditional jump, a closed graph and exact external target, unchanged input
registers and only an architectural x0 destination. `link=false` alone is
insufficient. Additional instructions, state changes, gaps and unknown targets
retain barriers. Missing summaries, cycles and further trampoline chains remain
explicitly unknown. Composition uses the same argument/stack substitution as
`call_writes`: entry sp is equal, but stack coordinates change owner. Original
store/load PCs, expression owners, widths, occurrence and every target barrier
are retained with the tail proof in `WriteOrigin`. Only this jump's transfer and
synthetic `OpaqueCall` barriers are removed. The pinned ROM
[validation](../riscv-stack/validation/call-spills/README.md) checks all twelve
`memcpy` stores through its trampoline.

The common value pass additionally proves a bounded byte-store footprint for a
straight unit-stride alignment loop. It uses the already prepared semantics,
CFG and converged all-path entry value, with no separate interpreter. Every
external entry must reach the shared masked test before the body. Exactly one
`+1` pointer definition, a power-of-two alignment mask (1, 3 or 7), the matching
nonzero back edge and all body/exit edges are required. Unsupported forms keep
the original memory observation. `StoreAddressRange` retains the initial value,
wrapping displacement, bound and structural proof; `function_writes` expands it
into ordinary `May` effects with `WriteProof::AlignmentLoop`. An open graph
discards those addresses. Store contents remain independent and unknown.
The [alignment validation](../riscv-stack/validation/alignment-writes/README.md)
proves three possible bytes for one real ROM store and transports them through
the existing trampoline/argument substitution; the other eleven store sites
retain their unknown-address barriers.

That same induction witness now supplies the pointer's value on its exit edge:
`(initial.wrapping_add(mask)) & !mask`, aligned with a forward modular
displacement in `0..=mask`, including zero iterations. A supplementary equality
component traverses the existing instruction CFG and intersects all incoming
paths. A matching zero-residue branch proves the same expression on the
already-aligned path. Register changes use the existing `evaluate` semantics;
copies preserve equality, other results lose it. Unknown inputs, calls, gaps
and open graphs retain uncertainty. No second loop recognizer or decoder is
used. `AlignedValue` records explicitly name an edge or instruction entry,
their function owner, original initial value, ordinary Add/And DAG expression
and the shared `AlignmentLoopProof`. They are conditional control-point facts,
not architectural `Value` writes or assertions that the point is reachable.
The scalar and memory observations retain their conservative contracts. The
[exit-value validation](../riscv-stack/validation/alignment-exit/README.md)
checks both real ROM paths and the merged pointer value while retaining every
existing store effect and call barrier.

`ResearchOptions::stack_calls` optionally retains an existing stack word when
`writes::stack_calls` prepared a complete disjoint call footprint from the
canonical caller index and callee `FunctionWrites`. Every target is required;
actual targets and exact sp/argument leaves are rechecked at the live call.
Callee stack coordinates are translated through actual sp. Byte ranges must
exclude the whole word without overflow or modular aliases. Unknown ranges,
internal calls and other barriers kill knowledge. Unknown stored contents have
an explicit `UnknownValue` barrier and do not prevent exclusion of a known
range. The bounded stack model, original load PCs, register clobbering and
return/behavior ports keep their existing contracts. No ROM memory contract is
selected automatically. The [fresh w1 validation](../riscv-stack/validation/call-spills/README.md)
records the selected ROM call's remaining loop-footprint barriers.
