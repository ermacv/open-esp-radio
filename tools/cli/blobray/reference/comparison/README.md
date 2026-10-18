# Comparison relations

Choose call, memory, branch, layout and effect relations.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Physical call capture and comparison

An invocation may set `observe_calls` to:

```json
{"include_tail":false,"argument_words":8,"overrides":[{"target":8192,"words":10}]}
```

`null` disables capture. Counts are explicit RV32 words (maximum 256); at most
128 unique target overrides are allowed. Eight words select a0–a7; later words
read the current private stack. Unknown and unavailable words stay explicit and
do not change code execution. Zero words selects targets only. `include_tail`
also captures x0 noncanonical jump candidates, including possible intrafunction
jumps; canonical returns and the root sentinel are excluded.

Set `relation.calls` to `true` on a comparison case and use the identical capture
profile on both sides. Physical targets and selected words compare in order with
selected MMIO/fence/delay events. Sites, SP and transfer/target kinds remain
provenance. `call-target` and `call-argument` differences identify the selected
call/effect index and differing values; changed ordering yields `event`.
Unknown selected words prevent MATCH. Captures stay retained when `calls` is
false. This profile does not infer semantic correspondence across different
addresses or interpret pointer arguments.

Capture happens before `observe-call` stops and before captured/model
dispatch. Target classification identifies the selected boundary, not execution
of its body. Each `call-transfer` plus its `transfer-argument` rows consumes
`max_events`, with whole-group admission and shared run limits. See
[physical call contracts](../../../docs/design/contracts.md#physical-call-observations)
for lifetime and claim scope.

## Internal physical timeline

Each invocation supplies `observe_timeline`:

```json
{"reads":true,"writes":true,"atomics":true,"branches":true}
```

Use all false to disable guest timeline capture. In the comparison relation,
`events.timeline` independently selects the same four channels; a selected channel
must be captured by both invocations. `max_events` and common run budgets apply.

Raw `memory` events retain instruction site and typed read/write/LR/SC/RMW data.
Read values distinguish known, unknown and unavailable. Writes retain their exact
width/value; atomics include ordering and old/new or SC outcome. `branch` records
include site, target, fallthrough and taken decision, including compressed branches.
Selected branch sites compare physically. Memory sites/origins remain provenance;
address, width and transaction values/order enter equality. No cross-layout or
pointer correspondence is inferred.

Existing call-model normal-memory effects enter these channels once from their
original records, including dynamic allocation as one `initialize-zeroed` span
for its nonempty accessible prefix. Unused capacity remains provenance; a zero-byte
allocation adds no memory transaction. Bulk initialization is distinct from
individual stores. Fetch, setup initialization, argument inspection and final
snapshots do not invent guest transactions. All selected memory/control observations
remain ordered relative to selected call/MMIO/fence/delay events. Different
intermediate states or read order can DIFF even when final memory matches.
Unknown/inaccessible reads or incomplete execution cannot MATCH. Raw excluded
observations remain available in the records. See
[internal timeline contracts](../../../docs/design/contracts.md#internal-timeline).


### Reviewed layout comparison

A layout projection is reviewed outside Blobray and supplied with an
[in-process comparison](../execution/README.md#in-process-verification). The
execution case's `relation.projection` selects it by the SHA-256 of its canonical
encoding, `in_process::projection_id`.

A projection specifies `vendor` and `replacement` endpoints, `fields`, `branches`,
`applicability` and `reason`. Each endpoint has an `entry`, plus `domains` such as
`[{"address":12288,"length":16}]`. An entry contains the executable `object`
by content, the exact `symbol` and `boundary` `{"kind":"code","address":4096}`,
with no name lookup or inferred extent; the executable must be one the target
maps. A field can map
different offsets:

```json
{"name":"counter","vendor":{"domain":0,"offset":0},
 "replacement":{"domain":0,"offset":8},"width":4,"count":1,
 "final_state":true,"timeline":true}
```

Both domains must contain their aligned field/array. Element widths 1/2/4/8 and a
nonzero count define the exact byte span, with a total 1 MiB projection limit;
aliases, missing capture, duplicate names and unsupported widths fail explicitly.
Capture final ranges in `observe_memory` and enable the selected timeline channels.
All declared final fields participate, including unchanged/unknown bytes. Padding
outside those fields is retained without being included in equality. Branch pairs
contain `vendor`/`replacement` objects with exact `site`, `target`, `fallthrough`;
only branch records at exactly those locations map, and the comparison does not
decode the instructions. Corresponding taken decisions must agree. Raw order, memory transaction kind/width/value and atomic outcomes remain
significant. Unmapped selected observations prevent MATCH.

All projections are finite reviewed assumptions bound to captured sources and entries.
A successful comparison establishes selected observations under those assumptions;
it does not establish general equivalence. See [projection contracts](../../../docs/design/contracts.md#reviewed-layout-projections).

### Reviewed effect comparison

An effect contract is reviewed outside Blobray and supplied with an in-process
comparison. A comparison case's `relation.effects` selects it by the SHA-256 of
its canonical encoding, `in_process::effect_contract_id`; all four MMIO read/write, fence and delay event channels must be enabled.

A contract contains exact `vendor`/`replacement` code endpoints, `rules`, `claim_ceiling`,
`applicability` and `reason`. Each rule contains `name`, per-side patterns,
`disposition`, `min_occurrences`, `max_occurrences` and `reason`. A pattern combines
`selector` with `value: {kind: "any"}` or `{kind: "exact", value: N}`.
Selectors are `mmio-read`/`mmio-write` with physical address and width,
`delay` with `micros` (a value or explicit `null` for all delays), or `fence` with
predecessor/successor masks. An optional `followed_by` selector restricts a pattern
to effects whose immediately next concrete effect on the same side matches it, and
an optional `preceded_by` selector to effects whose immediately previous concrete
effect matches it. An optional `occurrence: N` (from one) restricts it to the N-th
effect of that side its `selector` matches, counting every matching effect whatever
its rule: two samples of one register with different roles, such as a replacement's
own proof read before a read both sides share, get different rules. A rule
classifies each effect by the first pattern that selects it.
Per-side patterns cannot overlap: base selectors may overlap only when both
patterns carry non-overlapping `followed_by` or `preceded_by` selectors, or
different occurrences of the same selector.

`unclassified` is `incomplete` by default: every effect must be selected by a rule.
`unclassified: "required"` compares effects that no rule selects exactly, in order
and value, as if a required rule selected them.

Required rules compare identical observations. Omitted rules permit an ordered
subsequence of exact vendor effects; replaced rules require both explicit patterns
in corresponding order: a replaced effect keeps its position in the ordered
comparison, so its n-th vendor occurrence pairs with its n-th replacement occurrence
and every effect around it still compares in order; only the values within the
pair are not equated. Added rules validate replacement-only effects. Ignored rules
use identical patterns with `value: any` and retain matching effects on either side
as raw evidence without pairing them. Forbidden rules reject observed occurrences. Missing exercise and unclassified effects are
INCOMPLETE; known differences are DIFF. A zero minimum means required when observed;
an omitted rule always allows zero replacement occurrences. Rules reset per case.

`claim_ceiling: "selected-effect-equality"` permits only required/forbidden rules.
Omitted/replaced/added/ignored rules require `"reviewed-effect-refinement"`. Each saved
`CaseComparison` exposes this ceiling as `effect_claim` and its first unclassified
or unexercised obligation as `effect_gap`. A policy violation reports exact side,
raw event ordinal and rule ordinal in `difference`. MATCH with a refinement is
conditional on that reviewed policy; it is not physical equality or general
firmware equivalence. Incomplete execution cannot MATCH.

The records retain all raw evidence, including omitted and added effects.
Comparison composes with layout projections, selected internal timelines, return
words and final RAM. See
[effect contracts](../../../docs/design/contracts.md#reviewed-effect-contracts) for the
precise obligations, lifetime and limits.
