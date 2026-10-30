# Investigation workflows

The following profiles define current support; a use case missing here is not
provided. Blobray has no general equivalence proof, TUI, project repository,
cache, provider registry, or correspondence between library versions. The
[command reference](../../cli/README.md) owns syntax and format versions.

| Scenario | Status and boundary |
| --- | --- |
| Inventory of an archive or ELF | Implemented in process; partial inventory remains explicit |
| Whole-library function analysis | Implemented in process; blocked functions and gaps are outcomes |
| Library register accesses → review → source-model publication | Implemented; Blobray observations and source-owned hardware acceptance remain separate |
| Link a PHY entry with explicit ROM companions → analysis | Limited RV32 integer static profile, unsupported semantics remain gaps |
| Execute / compare implementations in process | Limited explicit integer scenario profile, scoped MATCH/DIFF/INCOMPLETE |
| Captured PHY I2C → compiled-production comparison | Native real scenario; 45 command-memory writes, descriptor/no-op leaves and both-host byte/field/reset transport under explicit bounded peripheral responses; independent MATCH/DIFF/INCOMPLETE expectations, no physical timing or RF claim |
| Current PHY calibration leaves → compiled-production comparison | Finite native matrix; TX-gain restore, forced gain, temperature conversion and post-init AGC with explicit domains and independent writes/returns; enclosing calibration remains outside this profile |
| Exact data ranges → bytes with provenance | Implemented for captured RV32 ELF bytes; relocations are counted, never applied |

### Contract verification links

| Contract | Implementation | Regression coverage |
| --- | --- | --- |
| Failed record growth releases the incoming payload without losing old records | [record memory](../../riscv-model/src/record_memory.rs) | `failed_record_growth_rolls_back_payload_and_allows_reuse` in the same module |
| Inventory keeps repeated members distinct and broken framing visible | [captured executables](../../blobray-application/src/captured.rs) | [inventory tests](../../cli/tests/inventory.rs) |
| Exact data ranges keep their provenance and reject unmapped ranges | [data export](../../blobray-application/src/data.rs) | [data tests](../../cli/tests/data.rs) |
| Section relocation admission supports small extents | [prepared object](../../blobray-artifacts/src/function.rs) | `ten_thousand_section_relocations_fit_small_function_capacity` in [analysis tests](../../cli/tests/analysis.rs) |
| A memory failure names the phase it happened in | [in-process limits](../../blobray-application/src/in_process.rs) | `value_state_capacity_failure_names_its_phase` in [analysis tests](../../cli/tests/analysis.rs) |
| Links validate every linker claim and report blockers | [linking](../../blobray-application/src/linking.rs) | [linking tests](../../blobray-linker/tests/linking.rs) |

## Workflow contract map

These routes define Blobray's user-facing responsibilities. An arrow transfers
a typed request or result, never a path the operation reopens.

| User question or action | Operation route | Governing contract | Acceptance scenarios |
| --- | --- | --- | --- |
| What is in these inputs, including missing/unsupported parts? | executable → artifacts inventory | [Artifact inspection](contracts.md#artifact-inspection), [identity](contracts.md#identity) | archive occurrence identity, thin archive coverage, malformed member coverage |
| What does this library's code do? | executables → library analysis with ISA ports → outcomes | [Analysis ports](contracts.md#analysis-and-verification-ports) | unknown source mappings, API and CLI parity, work exhaustion, bounded graph traversal |
| Can this archive entry be executed under this environment? | executables → link request → host linker → linked image → execution session | [Image linking](contracts.md#image-linking) | linker symbol resolution, companion linking, unknown source mappings, linker deadline |
| Does the compiled Rust implementation satisfy the declared comparison? | identified pair → linked images → sessions → verifier → records | [Comparison](contracts.md#analysis-and-verification-ports) | comparison outcomes, reference claim limits, session state isolation, companion linking |

A workflow requires all its relevant authority, resource and coverage contracts,
not just its successful path. Resource containment does not make a wrong symbol
association correct.

## Inspect inputs

The caller supplies vendor libraries or linked images as bytes. Inventory
reports every object, member ordinal, symbol, section, relocation, unsupported
content and missing thin member. A malformed member is an inventory outcome; it
cannot disappear from archive-wide coverage. A thin archive's members are given
as executables of their own.

## Analyze a library

Library analysis visits every function symbol of an executable section of
every given object and reports, per function, its records, control-flow
coverage and value-semantics summary, or why it is blocked. Code no function
covers is reported as a gap. Register accesses are the memory addresses these
functions access, filtered by optional ranges. A repeated request recomputes
its result; no computation is reused.

## Link and execute an archive entry

The caller selects the exact entry symbol by `SymbolId`. Supplying several
libraries does not prove their order or selection in an original firmware
build. The link request records inputs, roots, companions, layout and the
names left absent; the linker identity and contract are recorded in the image
manifest. Companion data and call definitions participate in the link.
Indirect callback roots are explicit inputs. Unresolved relocations, conflicting
layouts and unknown source mappings are blockers. Synthetic placement is
labeled in the manifest.

## Recover a table or coefficient

The caller selects an exact object and sized symbol or explicit section/image
range. Export returns the captured bytes and, per span, the relocation counts
that constrain their interpretation. Writable bytes describe initialization,
not runtime state. The observation is evidence, not an accepted interpretation.
Required recovered hardware tables and calibration coefficients retain source
identity, purpose, representation and applicability under the
[source policy](../../../../docs/source-policy.md). Their binary origin does not
justify dropping them or substituting another profile.

## Compare a Rust replacement

The caller selects identified compiled vendor and Rust executables, their entry
symbols, explicit environment/scenario inputs, a comparison relation and the
requested claim scope. A generated reference, shared production core and exact
production entry remain different evidence classes.

The comparison creates the declared execution sessions. Cold phases start
fresh; warm successors retain only session-owned state. Each invocation
declares its own entry. Executable models record their selected implementation
and applicability in the records.

Verification compares observations and coverage. `MATCH` answers the declared
relation over its stated scope; `DIFF` retains the counterexample; `INCOMPLETE`
retains missing obligations. None of these outcomes is silently relabeled a
product readiness result. The records name both compiled implementations,
scenario, models, producer, comparison policy, verdict and claim ceiling.

## Update a vendor library or interpretation input

A changed library is different content with its own identities. Reviewed
contracts and register models change through Git review. No correspondence or
automatic transfer of research between library versions exists.

## Acceptance scenarios

Each scenario states observable behavior Blobray guarantees; the table is not a
record of tests already run. Synthetic ELF/AR fixtures exercise contracts
without vendor inputs. Tests assert observable behavior and ownership rather
than internal layout or generated register constants.

| Contract | Scenario | Required observable result | Responsible boundary |
| --- | --- | --- | --- |
| archive occurrence identity | Two archive members have equal names and bytes; symbols repeat across tables | Every occurrence remains selectable by its ordinal | artifacts, domain |
| linker symbol resolution | Weak/common definitions, duplicate exports, library reordering and cyclic archive references | Inventory remains unchanged; the linked image and manifest reflect the actual linker semantics | linking |
| thin archive coverage | Thin archive with external members | Every member is reported with a missing-member diagnostic; the caller gives the members themselves | artifacts |
| malformed member coverage | Mixed or malformed archive members | Every payload occurrence has an outcome; supported subsets cannot claim complete archive coverage | artifacts, analysis |
| companion linking | Companion supplies data relocation and callback definitions | Every consumer links equivalent images for the same request | application, backend |
| unknown source mappings | Required source mapping or relocation is unknown | Affected claim is incomplete, never inferred from an equal label or placeholder value | artifacts, verification |
| linker deadline | A linker blocks or exceeds its deadline | The operation fails; the linker process is killed and reaped and the workspace removed | linking, host |
| work exhaustion | Exhaust work in an ELF table, member iteration or long name | Typed resource failure retains the last position | application, artifacts |
| scratch reuse | Exhaust scoped scratch, unwind a phase, then start another | Typed capacity failure; no escaped reference or stale ID; temporary capacity is reusable | computing module |
| deterministic accounting | Repeat with the same work policy and budget; overflow a counter | Deterministic accounting and checked arithmetic; no budget reset or wraparound | application |
| bounded graph traversal | Cyclic CFG/call graph or nonconvergent dataflow | Program traversal is iterative and bounded with declared completeness | analysis |
| retained allocation accounting | A sink retains records or a phase grows a buffer | Simultaneous allocations and result lifetimes remain charged | consumer, application |
| comparison outcomes | Missing behavior, known difference and fully discharged comparison | Typed INCOMPLETE, DIFF and MATCH with their coverage and claim scopes | verification |
| reference claim limits | Generated reference or model stands in for production behavior | Evidence keeps its limited class; exact production equivalence is not asserted | verification |
| session state isolation | Stateful phase is incomplete, or independent cases run in a different order | Dependent phases cannot consume unknown state; independent cases have fresh session state and no cross-case leakage | executor, verification |
| API and CLI parity | Issue equivalent requests through API and CLI/JSON | Same selection, results and diagnostics; only presentation differs | application, frontends |
| standalone build | Build generic Blobray without open-radio-specific code | No generic dependency on production code or chip hosts | crate dependency checks |
| dependency direction | Attempt a forbidden crate dependency | Dependency checks reject it | architecture checks |

## Contract enforcement

Crate-graph checks enforce dependency direction and standalone composition. They
cannot establish module authority or lifetime safety; behavioral tests cover
resource failures and claim semantics.

Capability tests establish what the supplied API permits, not an OS security
sandbox: code linked with `std` can still attempt ambient filesystem or process
access. Source/dependency review must reject such hidden access in computation
modules.

Tests use synthetic AR/ELF inputs for structural contracts. Nested/compressed
formats not supported by a parser test explicit coverage gaps, not nonexistent
decompression behavior. Real vendor cases supplement these tests without
replacing them or entering tracked fixtures. Documentation validation alone
establishes neither runtime guarantees nor hardware qualification.

## Register discovery and source publication

Run `register-accesses` over the libraries. It reports each function's
observations, unknown/alternative addresses, blocked functions and gaps. An
instruction access width never becomes physical geometry or hardware behavior.

To publish source, the independent register tool initializes explicit empty
peripherals or imports CMSIS-SVD into an unreviewed native model. A source
review supplies canonical physical identities, evidence, applicability and hardware
semantics in reviewed packs. Validation composes those with the selected model,
memory/ownership and PAC policies, then generates all four outputs. There is no
automatic Blobray-to-hardware acceptance or binary-derived write-semantics fallback.
See the [source-authoring commands](../../../registers/generate/README.md) and
[library register accesses](../../cli/reference/registers-data/README.md#library-register-accesses).

## Concrete execution profiles

The implemented device workflow declares exact ports and applicability in the shared
execution request, executes cold/warm phases and reads code and model outcomes.
Sequence/FIFO
obligations close at the declared lifetime; code return cannot turn missing model
participation into MATCH. The [device regressions](../../cli/tests/execution/devices.rs)
cover all eight mechanisms, ownership conflicts and closure;
[record validation](../../blobray-verification/src/records/models.rs) rejects forged
records. [Memory tests](../../blobray-application/src/devices.rs) verify release of model
payload/state and cancellation before response consumption.


The implemented external-call scenario composes admitted allocation, a warm-phase
read, an explicit call response, a device sequence and modeled delays in one
execution. [Call regressions](../../cli/tests/execution/calls.rs) check the actual
return and participation, all three verdicts, output ownership, stack words,
unknown values, response exhaustion, early goals and resource failure. Model
effects retain their
conditional scope; hardware qualification is separate.


## Selected final-state comparison (implemented profile)

Declare `observe_memory` on each invocation, including exact named address ranges.
Select per-case `relation` with return words, event channels and memory pair indices.
Use different physical addresses only through an explicit equal-length pair; this
scenario does not infer a layout mapping. Compare in process and inspect the
typed difference or selected unknown/unavailable bytes.

All selected bytes, including unchanged data, appear in the records. Excluded
observations remain available. An incomplete code phase's RAM snapshot is
intermediate evidence; successful execution coverage alone does not prove a
comparison when required outputs are unknown.

### Compare physical call boundaries (implemented)

Set each invocation's `observe_calls` to an explicit word/tail profile and select
`relation.calls: true`. For exact comparison use the identical profile on both
sides; target overrides select words only for that physical destination. Compare
in process, then read the `call-transfer`/`transfer-argument` groups and typed
difference. A zero-word profile compares target order without claiming argument
equivalence.

Compare call groups together with the required MMIO/fence/delay channels to retain
their relative order. Unknown selected words yield INCOMPLETE. `observe-call`
goals stop after boundary capture and before dispatch, so that prefix never proves
callee-body behavior. Exact physical comparison does not map renamed or relocated
semantic operations.

### Compare the internal physical timeline (implemented)

Set invocation `observe_timeline` flags for required normal reads/writes, atomics
and conditional branches. Select the corresponding `events.timeline` flags in the
case relation; both sides must capture every selected channel. All existing call/MMIO/fence/
delay selections preserve their order relative to selected internal observations.

A final-memory match can coexist with a timeline difference: an intermediate write,
load order, atomic outcome/order or branch choice may differ despite identical final
bytes and return. Excluded raw observations remain readable. Unknown/inaccessible
selected reads cannot MATCH. Call-model memory effects retain their explicit
assumption provenance; setup and inspection are excluded. Physical branch sites
compare exactly; cross-layout/control correspondence needs the reviewed layout
profile below, with no automatic normalization.

### Reviewed layout comparison — implemented finite profile

Capture both linked entries → declare explicit paired fields/branches in a
projection reviewed outside Blobray → select it by content in an in-process
comparison that receives it. Capture requested final ranges/timeline channels.
Compare keeps raw physical observations and explicit unknowns; unmapped selected
effects cannot MATCH. Different addresses can match only under the selected
mapping. Arbitrary type,
pointer-value and dynamic-path conversions are not part of this profile.

### Reviewed effect refinement — implemented finite profile

Capture both compiled inputs → identify exact root entries → declare explicit
required/omitted/replaced/added or forbidden MMIO/delay/fence rules in a contract
reviewed outside Blobray → select it by content with the desired layout, timeline,
return and final-memory relations → compare in process → inspect claim ceiling
and remaining obligations.

All raw effects remain evidence. A policy may deliberately relax physical
observations only under the reviewed refinement ceiling; it cannot discharge
unknown classification, missing required exercise or unfinished execution.
[Effect scenarios](../../cli/tests/execution/effects.rs) exercise content
selection, a missing contract, exact-value replacement, case
applicability and combined relations.
