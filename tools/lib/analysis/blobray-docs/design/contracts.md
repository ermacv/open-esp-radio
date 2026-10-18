# Interfaces and ownership contracts

This is the contract authority for Blobray. It describes implemented
boundaries; a capability it does not describe is not provided. The
[command reference](../../cli/README.md) owns CLI syntax.
[Architecture](architecture.md) owns components and [workflows](workflows.md)
owns supported use cases.

## Operations in process

Every operation runs inside its caller's process over executables the caller
supplies as bytes. Nothing is imported, stored, journaled or published: an
operation returns its result, or streams it to a caller sink, and keeps no
state after it returns. There is no project, revision, run record, worker
process or recovery protocol.

| Operation | Inputs | Result |
| --- | --- | --- |
| `captured::inventory` | One executable | `ArtifactInventory`: container kind, member completeness, every object's content identity, ELF tables and diagnostics |
| `library::analyze_library` | Library executables and the ISA semantic port | One `LibraryOutcome` per function symbol of an executable section, plus gaps, in input, object and symbol order |
| `library::register_accesses` | Library executables and optional address ranges | Streamed `RegisterAccess` records and a `RegisterAccessSummary` |
| `linking::propose_companions` | A `LinkRequest`, candidate companion executables and a linker | Companion symbols that close the request's unresolved names |
| `linking::link` | A `LinkRequest`, the executables it names and a linker | `LinkedImage`: image manifest, ELF bytes and the linker's map |
| `data::export` | A `DataRequest` and the executable holding its object | `DataExport`: exact selected bytes with per-span provenance |
| `audit::audit_targets` | One final image and forbidden ranges | Streamed `TargetAuditRecord`s and a `TargetAuditSummary` |
| `in_process::verify` | An `ExecutionRequest`, the executables of both implementations, selected contracts and projections | Records, verdict and completeness |

The CLI exposes the target audit and register accesses; the other operations
are library calls used by the vendor verification scenarios. JSON output is a
serialization of the same typed results, not a second implementation.

## Identity

`ArtifactId` is the SHA-256 of exact bytes. `Executable` pairs caller bytes with
that identity; a request names an executable only by content, and an operation
fails with `InvalidRequest` when a request names content it was not given.
Paths, file names and roles are presentation; they never identify an input.

| Identity | Meaning |
| --- | --- |
| `ArtifactId` | SHA-256 of an executable, a member's payload or a derived image |
| `ObjectId` | Executable identity plus standalone-object or archive-member ordinal |
| `SymbolId` | Object identity plus symbol-table kind, table section and entry index |

Archive member ordinals keep repeated names and repeated bytes as distinct
occurrences of one container content. A thin archive names external files
rather than holding members; its inventory reports every member with a
`MissingMember` diagnostic and the caller gives the member objects themselves.
A section offset identifies a location in an object section; an image address
identifies a location in a linked image and converts to a source location only
through that image's recorded mapping.

## Result assessment

Operation success and research completeness are separate. An operation that
returns has delivered a valid result; the result states its own coverage:

- an inventory is complete only when membership framing is intact and every
  object parsed without diagnostics;
- an analyzed function reports control-flow and value-semantics coverage
  separately, and a blocked function or a gap is an outcome, not a failure;
- a target audit reports `pass`, `fail` or `inconclusive`: `pass` covers
  statically resolved transfers only, and unresolved indirect transfers are
  counted separately;
- a comparison reports `MATCH`, `DIFF` or `INCOMPLETE` under its explicit
  relation, with execution completeness beside it.

Unknown structure stays explicit and never becomes a zero-byte observation.
The CLI returns success for valid partial research; a failed or inconclusive
audit returns failure.

## Resources and bounded computation

The caller owns every limit. `WorkingMemory` admits the capacity an operation
reserves; `RunControl` charges work units and checks the deadline; the
`in_process::Limits` control combines a work-unit limit, a wall-clock deadline
and the run position that a working-memory failure reports. A limit that is
reached fails the operation with `ResourceLimited`; nothing is truncated or
returned as a partial success.

| Resource | Owner and contract |
| --- | --- |
| Input bytes | The caller; operations borrow them and never reopen a path |
| Working memory | The operation reserves capacity before growth; exhaustion is a typed failure naming the phase, input and member |
| Work and deadline | One control across the whole operation, never reset at a phase boundary |
| Scratch | Scoped owners release capacity on success, error and unwind |
| Link workspace | A private temporary directory owned by one link and removed when it ends |
| Linker process | Owned by the host adapter; dropped processes are killed and reaped |

Working-memory capacity, work units, elapsed time and process memory are
distinct quantities; capacity admission is not an RSS meter or a claim that
every heap allocation was charged. Input-controlled recursion is prohibited:
explicit stacks and queues are bounded, cyclic traversals declare their
termination, and nested scans take part in work accounting. Deadlines are
checked every 256 checkpoints, so a wait on the linker notices an expired
deadline within its bounded poll interval. An opaque library call remains
outside cooperative cancellation.

A memory failure reports the rejected request size, the available capacity
and the last run position; it does not estimate the memory needed to finish.

## Artifact inspection

Artifact inspection returns every enumerated member and its parse outcome.
Broken member framing ends enumeration and records that the remaining
membership is unknown; a member whose payload is malformed is still a known
member, with a diagnostic and no ELF tables. Unsupported content contributes
explicit gaps. Symbol association and linker selection use different result
types: an inventory never decides which duplicate, weak or common definition
an original firmware link selected.

Prepared function and data views select `SHT_SYMTAB` or `SHT_DYNSYM` by
physical table kind, section and entry index; conventional section names are
not identities. One table of each kind is supported. Missing, duplicate or
mismatched tables and invalid indices fail explicitly. Dynamic symbols in
static RV32 objects remain ordinary metadata and grant no dynamic loading or
TLS support. Static relocations keep their own `sh_link` table independently of
the selected symbol.

Artifacts owns the prepared object: captured bytes, parsed ELF view, symbol
targets and section metadata live in one callback scope, and borrowed views
cannot escape it. Analysis owns normalized section references and logarithmic
offset, symbol and relocation-pair lookup. Whole-section HI/LO resolution
preserves missing and ambiguous outcomes.

## Image linking

A `LinkRequest` names its inputs and companions by content, the entry and
retained roots by `SymbolId`, the layout and the names deliberately left
absent. Application materializes the selected objects into the link workspace
under aliases it chooses (`i{input}-m{member}.o`), never under an original
untrusted name, and hands the linker adapter a fixed semantic invocation. The
host identifies the selected linker before running it; no adapter substitutes
another linker dialect. LLD and GNU ld keep distinct identities and archive
semantics. The ELF output extent is bounded by the remaining working capacity;
GNU ld's seekable output is bounded by `RLIMIT_FSIZE`.

Application validates every linker claim against the captured occurrences:
roots resolve to the requested symbols, placements map to materialized
members, and the linked ELF passes structural validation. Any unresolved
relocation or unmet root is a blocker; blockers are joined into one
`LinkBlocked` error rather than producing placeholder data. The manifest
records the request, linker identity and contract, ABI, ELF identity, entry,
roots, segments, source mappings and bounded linker diagnostics. A synthetic
analysis link is labeled as such: its addresses and member choices do not
prove original firmware placement or selection.

Companion proposal links the request as a trial with unresolved names
reported instead of failing, then resolves each unresolved name against at
most 16 static candidate executables in their explicit order. A name resolves
to the first candidate that defines it as a function or data object: a single
global or weak definition, or a local one only when no global or weak one
exists. Several definitions of the chosen binding, or none in any candidate,
leave the name unresolved; no companion is chosen by name similarity. A
companion is a captured definition, never a fabricated implementation, and a
link input must not itself define a companion name.

## Captured data

`DataRequest` names one object by `ObjectId`, an optional anchor symbol and
1–32 ranges: a symbol with its size or an explicit length, a section offset and
length, or an image address and length. The export returns the exact bytes of
each range, concatenated, with a span per range recording its section and
section range, file range, image address when the object is an image,
writability, the SHA-256 of its bytes and the counts of section relocations,
relocations overlapping the range and relocations of unknown extent.

Image addresses resolve only through loaded sections, each the bytes a load
segment carries; a range with no exact section fails. Writable section bytes are initialization values only.
Relocations are never applied. `NOBITS` ranges, overflowing lengths, symbols
of another object and empty or oversized range lists are rejected. A
successful export claims no completeness or comparison result.

## Analysis and verification ports

Library analysis visits every function symbol of an executable section with an
explicit or declared extent and produces a local graph with separately
qualified coverage. Its ISA port is injected; analysis never selects an
external symbol implementation, and there is no pass scheduler, provider
registry or computation cache. One prepared object serves all functions of an
object during one call.

Artifacts owns the borrowed `ImageMemory` view over the image's loaded
sections, each the bytes a load segment carries, with the caller's read-only
windows ([static RV32 images](../../../riscv/program/README.md)); load
segments themselves are only the execution profile's. Analysis reads constant
bytes through that narrow port and cannot load files or request mutable
machine memory.

Reviewed interpretations such as effect contracts and layout projections are
reviewed through Git outside Blobray and reach a comparison by content. A
changed body or ABI does not inherit an old contract merely because its name
matches.

Verification receives two identified compiled implementations, explicit input
and environment scenarios, a comparison relation and the allowed claim
ceiling. Each executor returns observations, coverage and model participation;
verification owns the verdict. `MATCH` requires all obligations of the declared
relation; a proven counterexample is `DIFF`; unavailable required behavior or
coverage is `INCOMPLETE`. A recorded difference is retained even when other
coverage is incomplete.

Execution-session state belongs to the scenario contract. Independent cases
start fresh; stateful phases carry only declared persistent state. Unknown
state after an incomplete phase blocks dependent phases. Models record
applicable preconditions and cannot silently replace the implementation under
comparison.

### Local value analysis boundary

The implemented [value and memory-effect contract](../../cli/reference/analysis/README.md#values-and-memory-effects)
extends local function analysis through an injected semantic port. The ISA
owner lifts operations and analysis owns fixed-point state. Unknown values,
incomplete effects and failed resource admission are distinct outcomes. No
hardware knowledge or executable memory environment is acquired implicitly.

### Implemented concrete execution boundary

Domain owns `ExecutionRequest`, observations and the injected
`Executor`/`ExecutionMemory` ports. Artifacts lends validated static ELF segments;
application owns the loaded executables, mutable session buffers, explicit device
state and all phase transitions. The RV32 backend owns concrete register/PC state;
verification alone computes the ordered-observation verdict and, separately,
validates the record structure of every run. None of those modules selects live
paths.

The invocation owns a bounded vector of known/unknown physical RV32 ABI words.
Domain validates stack geometry and computes entry SP; application initializes
stack arguments in its freshly owned stack, and passes eight optional register
words to the backend. Unknown argument slots override stack seeds. No register or
stack word becomes zero by omission, and setup emits no guest events. The caller
owns type/variadic lowering. The execution request format pins this interpretation;
producer identity pins both backend unknown-value semantics and stack environment.

The `byte-addressed-memory-1` environment admits ordinary unaligned 1/2/4-byte
RAM or ELF-backed data accesses entirely inside one mapping. Reads require all
bytes known/readable; stores validate the complete writable range before mutation.
No access stitches adjacent mappings together or wraps RV32. Timeline records keep
the original address and width; domain and record validation permit these ordinary
transactions while continuing to require aligned atomics. Misaligned MMIO never
consumes a response or splits into smaller operations. Instruction fetch alignment
is unchanged. This policy is part of `EXECUTION_ENVIRONMENT`, which keys reused
vendor results, so none comes from an aligned-only environment. It makes no hardware,
timing or concurrent-atomicity assertion. Regression ownership is
`execution/unaligned.rs` and `verification::records::timeline`.

`ExecutionMemory` also owns LR/SC reservation state, permission checks and indivisible
word updates. The backend supplies pure AMO arithmetic through a borrowed callback;
application invokes it once after admission/access checks, with no fallible step
between update and commit. No default read/write emulation is provided by the port.
The selected environment has one hart and no concurrent device writers, honors
program order, invalidates overlapping reservations and clears them between phases.
Atomic MMIO is unsupported and cannot invoke the register-bank model by fallback.
Atomic effects are ordinary captured guest behavior, not an expanded comparison
relation or a proof about multi-hart ordering.

Each case owns an explicit cold/warm transition and each invocation selects its
own captured-address-space entry. The first case is cold. Implementations have
separate mutable state: writable ELF and session RAM survive warm transitions;
phase RAM, stack and reservations do not. Models retain their own phase/session lifetime. RAM lifetime is part of its mapping
identity and cannot change while that mapping is live. Phase buffers release after
comparison. A warm successor to incomplete execution is blocked;
a cold case discards prior state/dependencies and starts a fresh chain without
erasing earlier incompleteness. All chains share one control and memory authority
within one request.
Physical execution goals belong to each invocation. Application validates that
symbols belong to the side's executables; artifacts resolves exact FUNC/NOTYPE
table entries against executable file-backed mappings. Goals resolve before any
session exists, and each executable's content identity is computed at most once
per request. Backend receives only a resolved goal and initial
register/stack state; it never resolves symbols or retains ELF metadata.

Returned, reached-symbol and observed-call are distinct completed outcomes. An early
return before a non-return goal is incomplete; a completed early goal permits a warm
successor but never promises the callee body ran. `complete` means all declared goals
and model closure obligations were met; it does not require every phase to return. Verification compares only the declared
observable prefixes; non-return goals cannot compare return registers. Record validation checks
outcome/goal kind and cold/warm dependency blocking without owning ISA interpretation.

A known difference survives other incomplete cases; a resource failure returns
no records. The implemented relation compares exact ordered
MMIO/fence/delay events and optionally one 32-bit return, with a caller-declared compiled
binding ceiling. It cannot claim arbitrary-domain or hardware equivalence. See
[concrete execution](../../cli/reference/execution/README.md#concrete-execution-and-comparison) for
request limits, memory initialization, schemas and unsupported behavior.

## Finite value alternatives

The RV32 values profile preserves at most eight canonical exact alternatives
at a register join. Domain owns nonrecursive leaves and validates 2..=8 sorted
distinct entries. Analysis owns the finite lattice and admitted,
operation-local set/index storage. Values and expression outputs borrow no set
owner after analysis. No global cache or additional allocator authority exists.

Joins retain a may-set; arithmetic applies to each bounded operand pair and
immutable image loads require all candidate reads to be modeled. Unknown inputs
absorb exact information. Distinct expression IDs and incomplete relocation
uppers are not exact alternative leaves. Overflow emits `SemanticGap::AlternativeLimit`
and unknown, never a truncated set or a chosen target. Work/cancellation admission
includes set lookup/growth and each candidate evaluation. Cycles converge under
the finite-height lattice; resource exhaustion fails the analysis.

Register accesses match each possible address without changing the analyzed
facts. CFG edges and original instruction/relocation records remain provenance;
sets do not encode path correlation, prove reachability or expand indirect
control flow.

Contract regressions: analysis `value_sets::tests` and
`values::tests::joins_and_loops_converge_independently_of_visit_order`.

## Register research and source publication

Register accesses read the functions of the given libraries. Candidate
addresses, instruction access widths and expression masks are observations,
not physical register/field declarations. Unknown addresses, alternatives,
partial analyses and blocked functions remain visible. No hardware meaning is
inferred from a coincident address; the reviewed register model owns register
identity.

The independent register owner initializes editable source models from explicit
peripheral geometry and imports CMSIS-SVD into the same native source format.
Neither operation accepts hardware claims. Imported XML is retained verbatim;
unsupported XML extensions are not promoted to native semantics. Reviewed source
assertions, applicability, evidence and publication policy remain explicit inputs
to validation and the existing four-output publisher. Blobray does not acquire
register-generation or production dependencies.

### Implemented device ownership and completion

Concrete platform scenarios may supply captured guest setup code and register-input
shims through ordinary execution targets. Their bytes and input words are retained
dependencies, not implicit engine initialization. A warm phase can consume setup
writes to captured writable ELF memory; a RAM seed cannot overlap or replace that
mapping. Unknown register copies and spills propagate as unknown values, even
into filled stack memory, and stop execution only where they decide control flow,
form an address or reach a device. The explicit-input regression in
[session tests](../../cli/tests/execution/sessions.rs) checks both paths. The
[PHY I2C scenario](../../../../verification/esp32s31/README.md#captured-i2c-command-memory-comparison)
records this narrower input/observation scope and uses shipping HAL/PAC behavior.
Chip-specific setup, addresses and expectations stay with the verification owner.

`DeviceDeclaration` is a caller assumption with explicit applicability, stable
content identity and phase/session lifetime. Application `devices` owns configuration
clones, admitted mutable banks, transcript cursors and an exact sorted port index.
The ISA backend sees only memory ports; it neither resolves models nor invents a
response. Atomic accesses never fall back to device behavior. Model geometry cannot
claim a mapped byte or the gaps between indexed ports.

Each phase snapshots cumulative participation separately from its code outcome.
Phase instances close there; session instances close at the last phase of a chain.
Warm continuation omits live declarations; cold reset or expired phase ownership
permits fresh ones. Closure releases owned configuration and mutable data, while
bounded index capacity may remain for reuse. All allocation, traversal and access
work shares the existing operation budget, with no global cache or new runtime.

`ModelObservation` distinguishes open, complete and incomplete obligations. Required
Sequence reads use bounded value/count runs with a checked logical total;
application retains a cursor and remaining count without expansion. Record validation checks
logical consumption, and identity binds the encoded runs. Sequence/FIFO values
must be consumed at closure; failed accesses preserve an issue.
A returned goal cannot override incomplete environment evidence. Verification
requires both code and due model obligations for MATCH. Record validation checks identity,
monotonic counts, declared transcript totals, exact closure and required observation
presence; it rejects MATCH with unmet obligations without executing model semantics.
Models and code observations are recorded together. Full current
mechanism syntax and claim limits belong to the [operator reference](../../cli/reference/execution/README.md#concrete-execution-and-comparison).

Packed-command banks follow the same owner. Domain defines bounded wire geometry,
seeded cells, script inputs and command accounting; application `command_bank`
owns admitted per-port pending commands and shared mutable cells. `devices` owns
installation, exact port lookup, access work and closure. Only a ready read commits
a staged write. Reset can explicitly abort one port's pending command, preserving
shared cells and script cursors. No chip encoding, polling loop or RF algorithm is
inside the generic mechanism. A model with pending commands cannot close complete.

Record validation checks command/sample conservation and phase deltas against
the declarations. Its accounting checks cannot claim the ISA or device responses
were re-executed.
The [native command tests](../../cli/tests/execution/command_bank.rs) cover
warm completion, incomplete closure, gaps and capacity; domain, application and
record-validation tests cover geometry, independent shared-bank values,
reset, sample consumption, cancelled admission and forged evidence counters.


### Implemented external-call boundary

`CallDeclaration` owns exact target/boundary, caller applicability, physical argument
width, immutable response sequence and phase/session lifetime. Application
`external_calls` owns admitted copies and cursors; `execution_calls` validates full
responses and mutates only session-owned normal memory. Device and call models have
separate explicit ports and ID namespaces, and share completion/closure semantics.
No global response registry participates.

The backend invokes the required `ExecutionMemory::call` port at eligible transfers,
after checking observe-call goals. An unselected transfer executes captured code;
a selected response has explicit returns, output/allocation/delay effects and ABI
clobbers. Root entries and canonical returns cannot be modeled accidentally. Model
responses never constitute evidence that replaced code executed. All participating
assumptions remain in the immutable request and model identities.

Output geometry/ownership, allocation freshness and required argument values are
validated before response mutation. Resource failure during admitted effects fails
the request. Allocation capacity is owned until its declared lifetime
ends; only the requested zero-initialized prefix is accessible. Model responses and
memory allocations have independently declared lifetimes. Warm continuation cannot
redeclare a live model or reseed live allocation storage through ordinary RAM.

Fixed-size streamed events retain modeled boundary, arguments, effects and returns.
Event capacity is admitted for the full successful response before mutation. Record
validation checks selected bindings, ordered arguments/effects/returns, exact response consumption
and closure; missing/forged evidence cannot complete the run. The explicit relation
selects MMIO/fence/delay, return words, final memory, physical/reviewed calls and
internal timeline channels; excluded model records remain available for inspection. Delay values are assumptions, never wall-time or
hardware timing evidence.

## Selected final memory and comparison relations

Every comparison case owns a `ComparisonRelation`; single-implementation cases
use none. It selects independent low/high return words, ordered MMIO read/write,
fence and delay channels, and physical `MemoryPair` indices. The relation must
select at least one domain. Return words require Return goals on both sides. No
other phase's relation or old request flag supplies a default. The request retains
each selection.

`Invocation.observe_memory` names exact nonempty disjoint normal-memory ranges.
A pair selects one range per side with equal byte length. Names and pair indices
are explicit; matching names/addresses do not infer corresponding layouts. There
are at most 128 selections and 1 MiB selected bytes per invocation. Bounds are
validated before execution; selections never truncate silently. Observations
may be retained without being selected for comparison.

Application snapshots every requested byte at the phase stop, before releasing
phase RAM, allocations, tables or stack. It accesses readable normal-memory views
without invoking MMIO/device callbacks. Availability distinguishes readable mapped
bytes from inaccessible/unmapped bytes; knownness distinguishes initialized values
from unknown contents. Selected unchanged bytes are included. Canonical fixed-size
16-byte chunks identify selection/offset/length and both masks; masked-out bytes
are stored as zero only as an encoding rule, never as an observed value. Blocked
phases execute and capture nothing, with explicit blocked outcome.

Session admits the snapshot vector before allocation and owns its reservation
through comparison. Recycling drops the snapshot before
releasing capacity; a following phase cannot accumulate previous snapshots. These
records are separate from guest events and do not consume `max_events`; their
independent byte bound and shared memory/work/disk budgets still apply. Capture
failure fails the whole request. No extra memory cache or lifecycle owner exists.

Record validation checks chunk order, exact range coverage, canonical masks/bytes, phase/side
ordering, blocked absence and selected-knownness before accepting a comparison
MATCH. Missing or forged chunks are integrity failures. A difference descriptor
must name a selected event/return/memory domain. Record validation checks structure;
verification remains the sole owner of comparison computation.

Verification compares selected event prefixes and known returned words, then paired
memory bytes using bounded selection lookups. Known selected differences dominate
other unknowns. Memory snapshots from unfinished code goals cannot establish a
DIFF in completed final states; they remain retained intermediate observations.
When both code goals completed, any known differing selected byte proves DIFF even
if another selected byte is unknown. Selected unknown/unavailable bytes prevent
MATCH. Full goal/model completion plus equality of every selected observation is
required for MATCH; excluded observations cannot silently enter the relation.

`ExecutionObservation::completed` and execution-scenario completeness still mean
code goals and due environment obligations were met. They do not claim that all
retained memory/returns are known. Unknown selected outputs can therefore yield an
INCOMPLETE comparison beside completed execution coverage. Comparison verdict,
operation state and scoped execution coverage remain independent machine fields.
A failed phase blocks warm execution; an unknown selected output or comparison
DIFF alone does not change the session's successful code-goal transition.

### Physical call observations

`Invocation.observe_calls` explicitly selects transfer capture for this phase.
`null` disables it; a profile declares `include_tail`, `argument_words` (0–256)
and at most 128 unique aligned physical target overrides. Zero words means
target-only observation. This is physical RV32 word selection, with no inferred
signature, variadic lowering or pointer equivalence.

The backend invokes the observation port on x1/x5 transfers and optionally x0
noncanonical transfers. Canonical x1/x5 returns and the root return sentinel are
excluded. Opt-in x0 transfers are tail candidates: this convention also includes
intrafunction jumps and does not prove function boundaries. Capture precedes
observe-call goal completion, model dispatch and callee
code. It never invokes the destination. A `call-transfer` header retains physical
site/target, direct/indirect form, tail flag, current SP, word count and selected
target kind (captured code, call model or unavailable). Target kind
describes the available boundary, not proof that its body executed; the request,
model identities and adjacent dispatch evidence retain that distinction.

The following ordered `transfer-argument` records contain a0–a7 then words from
the current private stack. Unknown registers/bytes remain unknown. Missing SP,
misalignment and outside-private-stack accesses remain unavailable with a reason.
Capture never redirects a stack read to RAM or MMIO, and unknown captured data
alone does not stop code. Each group admits header plus all words against shared
event capacity before emission. Application owns the admitted sorted override
index for one phase and releases it at phase completion; retained event capacity
remains charged through comparison. Work, memory, deadline or disk
failure fails the entire request.

`ComparisonRelation.calls` selects exact ordered physical targets and configured
words, interleaved with the selected MMIO/fence/delay channels. Both invocations
must select the identical capture profile. Sites, SP, transfer form and target
kind remain provenance rather than implicit equivalence conditions. An event
index counts selected calls/effects, not individual argument rows. Different
targets or known selected words yield typed `call-target`/`call-argument`
differences; different call/effect order or completed stream lengths yields
`event`. Unknown/unavailable selected words prevent MATCH; known differences
remain DIFF despite other unknowns. Completed code/model obligations remain
required. Excluding calls does not erase their evidence.

Record validation checks requested geometry, contiguous word groups, unavailable reasons,
observe-call boundary presence and selected knownness before admitting MATCH.
It does not replay ISA semantics to authenticate the physical trace. Verification
borrows grouped slices without allocating or rescanning preceding calls. The
[capture regressions](../../cli/tests/execution/capture.rs) exercise application,
word/effect differences and resource atomicity;
[record validation](../../crates/verification/src/records/capture.rs) rejects malformed
groups. Layout projections require their explicit reviewed relation below; this
physical profile infers none.

### Internal timeline

The invocation explicitly selects normal-memory reads/writes, atomics and
conditional branches for capture; the relation independently selects those
channels for comparison. Selected channels must be captured by both sides.
Application owns admitted event storage and physical normal-memory ownership;
the backend supplies the current instruction identity and branch decision.
Instruction fetch, argument inspection and setup initialization do not become
guest data transactions. Declared call-model memory effects retain their existing
records and enter selected normal-memory channels once. Memory site/origin remain provenance; physical address, width,
values and atomic outcome/order are comparison inputs. Conditional branches retain
physical site, target, fallthrough and taken decision. No cross-layout or branch
correspondence is inferred by this profile.

Unknown or inaccessible owned reads stay explicit. Failed instructions/accesses
also retain their execution gap and cannot establish completed equality. Successful
stores, LR/SC/RMW and declared model effects are ordered with selected calls/MMIO/
fence/delay. Setup/inspection cannot add extra guest effects. A known prefix
difference can establish DIFF; equal final memory cannot erase a different selected
timeline. Capture failure fails the entire request. Reviewed projections use
the separate explicit contract below.

`Invocation.observe_timeline` contains explicit `reads`, `writes`, `atomics` and
`branches` booleans. `ComparisonRelation.events.timeline` independently selects
these channels and must be a subset of each side's capture. Raw memory events
carry instruction PC plus a fixed-size typed transaction. Reads retain known,
unknown or unavailable values. Successful normal stores retain truncated values
at widths 1/2/4. LR records its old word; SC records operand and success/failure;
RMW records old/new words. Atomic ordering bits are explicit program-order inputs,
not a multi-hart timing guarantee. Invalid/unowned accesses stop with a gap rather
than inventing a normal-memory owner or using an implicit device fallback.

`observe_timeline.written` is not an event channel: it reports the persistent
bytes a phase stores into writable image segments, session RAM and session
allocations as ascending, coalesced `WrittenRange` records after the phase's
final memory, without values or order. Stack and phase-lifetime memory are
excluded. A phase holds at most `MAX_WRITTEN_RANGES` ranges; a store
that would add one more stops the run as resource-limited. Record validation
requires the capture flag, record order and ascending coalesced ranges. Clients
use the ranges to find written state that no relation compares.

The executor's `instruction` port identifies memory sites independently of shared
progress. A phase clears this identity and capture flags. Conditional branch
records include site, target, fallthrough and taken decision for both ordinary and
compressed instructions. Fetch, setup seeds, argument inspection, final snapshots
never duplicate guest transactions. Existing `CallOutput` writes enter the
selected normal-memory relation once via their original records, retaining model
provenance.
Dynamic `Allocation` records enter writes as one `initialize-zeroed` span for
the nonempty requested prefix, never the unused capacity. A zero-length request
retains allocation evidence but has no memory transaction. Bulk initialization is
an explicit transaction, not implicitly coalesced with individual stores.

Event storage remains admitted once per session and recycled per phase. Memory
writes/atomics check event capacity before mutation; RMW admission precedes its
pure update callback. Failures during subsequent evidence handling fail the
request. Record validation checks capture authorization, instruction/branch geometry,
transaction widths/values and selected-knownness; it does not reconstruct ISA
execution from the transcript. A claimed completed outcome plus an unknown selected
read still cannot admit MATCH. Verification borrows transactions and compares
normal-memory values without including source PC/origin as an implicit equality
condition. Branch comparison is exact physical control comparison; differing code
addresses require an explicitly selected reviewed projection.

[Timeline regressions](../../cli/tests/execution/timeline.rs) cover all widths,
AMO/orderings and LR/SC, ordinary/compressed branches, intermediate differences
with equal final RAM, call-model effects, phase capture, unknown/unreadable
memory and capacity.
[Record validation](../../crates/verification/src/records/timeline.rs) rejects malformed
or unrequested transcripts and invented MATCH; application verifies capacity before
atomic callback/mutation. No elapsed-time or real hardware equivalence is implied.


### Reviewed layout projections

`LayoutProjection` owns two exact captured code entry occurrences,
explicit normal-memory address domains, paired byte fields and conditional branch
coordinates, with applicability and reason. Entry boundaries must be physical code
symbols in retained linked executables. Domain/field declarations are reviewed
runtime layout assumptions, not claims that the ELF contains initialized data there.
The session's normal-memory permissions and byte knownness remain authoritative.
The executable content and entry of each side must match its endpoint; changing
an entry or executable cannot silently reuse a projection.

At most 128 fields, domains per side and branches are accepted. Domains are bounded,
nonempty and nonoverlapping, and every domain has a declared field. Field names are
unique labels, never resolvers. Each field declares a domain index and offset per
side, equal element width 1/2/4/8, a nonzero `count`, and `final_state`/`timeline`
participation. Checked width × count defines the field/array span; all projected
spans together are limited to 1 MiB. Both aligned field
ranges must fit their domains; overlapping aliases or unused fields are rejected.
All declared final fields require captured snapshots containing every byte and
cannot overlap a physical whole-selection comparison. Unknown padding outside the
selected fields stays in evidence without claiming it is equal.

Branches declare site, target and fallthrough on each side. Duplicate branch sites
and invalid geometry fail validation; the comparison maps only branch records at
exactly the declared coordinates and does not decode the instructions. Projected
branches compare the paired identity and the same taken decision; predicate inversion
or different dynamic path shapes are not inferred.

The case selects one projection in `ComparisonRelation.projection` by the digest
of its canonical encoding. The projection is reviewed outside Blobray and supplied
with an in-process comparison, which validates its applicability to the request.
All field/branch scopes declared by that projection participate; a final-only
profile is explicit.

Verification borrows the selected contract and observations. Final fields compare
all selected known bytes at completed goals. Timeline memory is mapped to field and
within-field offset, retaining transaction kind, width, values and atomic order/outcome.
An effect must fit wholly inside one selected field/array; bulk allocation initialization
is not implicitly split or coalesced. Pointer values are compared exactly, never
rebased because their storage address moved. Branch/memory observations stay in the
same ordered stream as selected calls/MMIO/fence/delay. An unmapped selected memory
or branch observation makes equality unknown rather than disappearing or falling
back to physical equality. Known mapped differences remain DIFF; unknown bytes or
unfinished code/model obligations cannot MATCH.

The verifier traverses array snapshots with advancing borrowed byte cursors, without
rescanning their chunks per byte. Raw physical evidence and excluded bytes remain intact. This finite profile claims
selected observation equality under reviewed assumptions, not universal ABI, type,
algorithm or hardware equivalence. Arbitrary width conversion, pointer-value mapping
and path normalization are outside this profile and are never approximated.

[Projection scenarios](../../cli/tests/execution/projections.rs) cover content
selection, a missing projection, invalid geometry, final fields,
unknown padding/fields and missing capture. [Verifier tests](../../crates/verification/src/projection.rs)
check ordered mapped memory/control and unknowns.

### Reviewed effect contracts

`EffectContract` binds exact captured vendor/replacement code entries. It is
reviewed outside Blobray and supplied with an in-process comparison, which
validates its structure and applicability to the request; verification owns the
comparison. There is no policy-name lookup, implicit executable or inferred
replacement.

Each case selects one contract in `ComparisonRelation.effects` by the digest of
its canonical encoding. Selection
requires all four concrete MMIO read/write, fence and modeled-delay channels. Internal
memory/branch, call, return and final-memory selections remain independent and
compose with the effect relation. A selected contract the comparison was not
given is an error; absence
of an effect selection retains the explicitly selected physical comparison.

The finite profile permits up to 128 uniquely named rules. Every rule has a reason,
explicit per-side patterns and minimum/maximum occurrence counts. MMIO selectors
use aligned physical addresses and widths 1/2/4; fence selectors use the captured
predecessor/successor masks; delay selectors use a duration or explicitly all
modeled delays. Patterns may require an exact value or allow any value. Selectors
on each side must not overlap, including overlapping MMIO spans and wildcard
versus specific delays. There is no first-rule precedence. Fence patterns have no
separate value constraint because their complete observation is the masks.

A pattern may add a `followed_by` selector: it then selects an effect only when
the immediately next concrete MMIO, fence or delay effect on the same side matches
that selector. The end of the case has no successor. Two patterns whose base
selectors overlap are distinct only when both carry non-overlapping successor
selectors. Verification evaluates the successor with a one-event lookahead; no
other context or history is expressible.

`unclassified` selects the treatment of effects that no rule selects. The default
`incomplete` keeps the every-effect-classified policy below. The explicit
`required` policy compares such effects exactly, in order and value, as though a
required rule over their own identity selected them; it adds no count obligation.

| Disposition | Ordered comparison obligation |
| --- | --- |
| `required` | Identical patterns on both sides; every selected occurrence compares exactly, including values and order among other selected observables. |
| `omitted` | Identical patterns; replacement may omit vendor occurrences. Every retained replacement occurrence must equal a vendor occurrence in order. Removing an occurrence never licenses changing its value or moving it across a selected call, memory access or other effect. |
| `replaced` | Both explicit patterns must hold; corresponding occurrences compare by the same rule identity and order. Values are constrained by the individual patterns, without an implicit physical equality requirement. |
| `added` | Only a replacement pattern; matching replacement effects discharge its count/value obligations and remain raw evidence outside the paired stream. |
| `ignored` | Identical patterns with any value and a positive maximum. Matching effects on either side are implementation plumbing: they remain raw evidence outside the paired stream, discharge only count bounds and never shift alignment. |
| `forbidden` | Identical selectors on both sides, unconstrained value and zero counts. Any matching observed occurrence is a known violation. |

A minimum of zero means required when observed. Otherwise missing exercise is
INCOMPLETE, including when both complete runs have no occurrence. The replacement
minimum of an omitted rule is always zero. Counts reset for each case, including
warm continuations. Excess counts, forbidden effects and violated exact-value
constraints establish DIFF from actual observations, even if execution stopped
later. A completed shorter required stream also establishes a difference; an
unfinished prefix cannot establish equality or an absent future effect.

Under the default policy every raw concrete effect is classified. Unclassified observations produce
INCOMPLETE and stop positional alignment: an unknown omission/replacement must
not shift subsequent positions into a fabricated mismatch. Independently known
return/final-memory differences and directly observed policy violations can still
establish DIFF. Full policy accounting precedes alignment, so a violation after an
unclassified event remains visible. Neither a satisfied contract nor excluded
observations can turn unfinished execution or unmet environment obligations into
MATCH. Raw excluded effects, exact inputs and model participation remain in the
records.

`CaseComparison.effect_claim` exposes the selected claim ceiling even for DIFF or
INCOMPLETE. `selected-effect-equality` permits only required/forbidden rules;
`reviewed-effect-refinement` is mandatory for omitted/replaced/added/ignored rules. MATCH
under the latter means the reviewed refinement held for these concrete cases,
not physical event equality, general equivalence or hardware qualification.
`effect_gap` identifies the first vendor-side gap, then replacement-side gap:
an unclassified raw event ordinal, otherwise an unexercised rule ordinal.
`EffectViolation` includes side, raw event ordinal, rule ordinal and cause. Gaps
are retained alongside an independently established difference.

Verification borrows the resolved contract and raw observations. Bounded per-side
counters have no expanding heap allocation; every rule scan consumes the shared
work budget. [Effect scenarios](../../cli/tests/execution/effects.rs) and
[verifier tests](../../crates/verification/src/effects.rs) cover these boundaries
and policy composition.
