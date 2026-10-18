# Blobray verification

`blobray-verification` compares concrete observations through domain values and
validates the records of a run. It has no filesystem, executor or selection
authority. Application
supplies observations, the exact case relation and shared operation control.

The verifier compares explicitly selected ordered
MMIO read/write, fence and modeled delay channels; selected low/high return words;
exact paired final normal-memory ranges; and opt-in ordered physical call targets/words. Physical range lengths must agree;
addresses may differ only because the caller selected that pair. No pointer, ABI
or layout equivalence is inferred. Unselected evidence remains with the caller.

Known event/return differences yield DIFF even with other unmet obligations. A
code-goal-completed shorter event stream contradicts an observed extra event;
an unfinished shorter prefix cannot establish that length difference. Selected
unknown returns or memory bytes cannot establish equality. A known differing final
byte proves DIFF when both code goals completed; snapshots at unfinished stops are
intermediate evidence and cannot prove different completed final states. Equal
selected data can MATCH only with both code goals and due model obligations met.

`CaseComparison.difference` identifies the selected event index, return word, or
memory pair/byte offset and values. Missing execution/model obligations remain in
adjacent observations. Every compared item consumes shared work; exhaustion is an
error, never a verdict. The application aggregates DIFF before INCOMPLETE and
returns the records to its caller.

Memory lookups use the ordered selection/chunk index without cloning snapshots.
Unknown and unavailable bytes are distinct retained facts. Elapsed time and
abstract service internals are outside this implemented relation. Results concern explicit cases and the declared
compiled binding; they do not establish whole-domain equivalence or qualification.
See [selected comparison contracts](../../docs/design/contracts.md#selected-final-memory-and-comparison-relations).

A relation that compares calls compares every captured call physically: the
same target and every captured argument word, with raw evidence retained. It
infers no ABI, layout or pointer normalization.

`events.timeline` selects normal reads/writes, atomics and conditional branches.
Guest memory records and declared call effects share typed normal-memory
comparison without duplicate bookkeeping events. Dynamic allocation compares one
zeroed requested span, excluding inaccessible capacity; empty spans add no memory
effect. Bulk initialization is distinct from individual stores. Memory sites/origins remain
provenance; branches compare exact physical control coordinates. Unknown reads
cannot establish equality, and identical final RAM does not erase a timeline
DIFF. All selected channels remain interleaved in one ordered comparison stream.

Reviewed layout projections map exact byte fields/offsets and conditional branch
coordinates while retaining order, unknowns and atomic semantics. Unmapped selected
effects prevent equality. Final fields compare complete selected known bytes; unknown
padding is retained outside the explicit selection. Projected call words compare
exact 32-bit values at reviewed positions. No pointer-value or type conversion is
inferred. The verifier borrows contracts and observations; the caller supplies
reviewed contracts by content.

Reviewed effect contracts classify every concrete MMIO/delay/fence observation.
Required effects remain exact; optional omissions retain an ordered exact
subsequence; replacements pair explicit patterns; additions and prohibitions keep
independent exercise/value/count obligations. Unknown classification stops alignment
without manufacturing a shifted mismatch. Raw evidence remains with the caller.
The result exposes the review's claim ceiling and first policy gap; a refined MATCH
never asserts physical equality. Shared bounded domain counters let record
validation check that accounting independently. `classify_effects` returns the
selection a contract gives each effect of one side, so a caller can count the
effects each rule selects. See [effect contracts](../../docs/design/contracts.md#reviewed-effect-contracts).

## Record validation

`validate_records` checks the records of one run against its request, the
effect contracts and layout projections the request can select, and the
verdict and completeness the run reported. It shares no code with `compare`:
it replays the record stream structurally, per case and side, and checks event
capacity and the requested physical timeline, captured call arguments, final-
memory chunk geometry, device and call model identity, participation and
closure, goal and blocking outcomes, effect accounting and coverage order. A
`MATCH` whose case left a selected observation unknown, an obligation unmet or
a selected effect outside the reviewed contract or projection is an
`Integrity` error. It never re-executes instructions. Application applies it to
every in-process run; see
[record validation](../../cli/reference/execution/README.md#record-validation).
