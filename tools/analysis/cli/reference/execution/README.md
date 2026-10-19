# Concrete execution and comparison

Run explicit RV32 scenarios with selected inputs, device models and comparison observations.

[Choose a task](../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Concrete execution and comparison

Execution runs inside the calling process through
[`oer_analysis_blobray_application::in_process`](#in-process-verification): the caller
supplies a request, the ELF bytes of its targets' sources and the effect
contracts and layout projections its relations select, and receives the
records, the aggregate verdict and completeness in memory. No CLI command
participates, and nothing is retained.

A request without a replacement executes one implementation; a comparison
supplies both and an explicit binding class. A request has the following shape
(replace the executable identity and entry with the caller's own):

```json
{
  "schema": 25,
  "vendor": {
    "executables": ["ELF_SHA"],
    "abi": "riscv-integer",
    "stack": { "address": , "length": 65536, "fill": null, "bytes": [] }
  },
  "replacement": null,
  "binding": null,
  "cases": [{
    "name": "one-explicit-case",
    "reset": "cold",
    "relation": null,
    "vendor": { "entry": , "goal": {"kind":"return"}, "arguments": [0, 0, 0, 0, 0, 0, 0, 0], "memory": [], "models": [], "calls": [], "observe_memory": [], "observe_calls": null, "observe_timeline": {"reads":false,"writes":false,"atomics":false,"branches":false} },
    "replacement": null
  }],
  "max_events": 4096
}
```

A target names, by the SHA-256 of their bytes, the standalone static RV32 ELF
executables whose code and data segments it maps, in load order: one to 65
(`MAX_TARGET_EXECUTABLES`), each at most once. A captured input and a prepared
image are both such executables. The caller supplies the executables; a target
naming one it did not supply fails. Overlapping segments are rejected; linker
absolute definitions alone do not supply executable bytes. Execution never
rediscovers origins or substitutes another symbol implementation.

For comparison, supply a second target in `replacement`, set `binding` to
`production-entry` or `shared-core`, and supply each case's replacement invocation.
Binding is the caller's declared relationship to production; a label does not
authenticate that relationship or grant qualification. Captured bytes, entries,
scenarios and implementation identities remain in the evidence. Side-specific
arguments and layouts are explicit; the verifier does not infer their equivalence.

An invocation supplies zero to 256 already lowered RV32 integer ABI words in
`arguments`. Numeric entries are known; `null` and omitted register words are
unknown. The first eight words initialize a0–a7. Remaining words occupy successive
little-endian 32-bit slots at entry SP, beginning at offset zero. The stack grows
down; its top must be 16-byte aligned. Application reserves the stack argument area
rounded up to 16 bytes and places SP at its start. That area must fit the declared
stack; bytes below SP remain available for callee frames. With nine words, SP is
stack top minus 16, and word nine is at SP. With no stack words, SP is stack top.
These placements follow the [RISC-V integer psABI](https://riscv-non-isa.github.io/riscv-elf-psabi-doc/).

The words are physical ABI slots, not a C type description. Clients explicitly
lower wide scalars/aggregates, indirect values and variadic alignment, including
padding slots; the executor does not guess argument types. An explicit unknown
stack word overrides even known stack seed bytes/fill. Alignment padding and
remaining stack bytes retain only the declared seed initialization. Argument setup
creates no guest memory/MMIO event. x0 is zero and ra is a reserved unmapped return
sentinel. Other integer registers begin unknown. An unknown value propagates:
arithmetic on it, a load of unknown bytes and a store of it to memory yield
unknown values, as compiled code may copy uninitialized padding, and returned
or compared unknown values leave the comparison `INCOMPLETE`. An unknown value
that decides execution or leaves the guest (a branch condition, a jump target,
a memory address, an atomic operand, a device write or a store a recorded
write timeline would observe), an inaccessible memory access
or an unsupported instruction ends that phase with a typed `incomplete`
observation and its PC. The current integer executor supports RV32IMAC arithmetic, branches,
loads/stores, direct/indirect jumps, word atomics and ordinary fence events. FP,
CSR/privileged execution, syscalls, dynamic loading and TLS are unsupported.
Declared float ABI flags do not silently select floating-point execution.

Ordinary RAM and ELF-backed data allow unaligned halfword/word loads and stores
within one mapping, using little-endian byte order. Every loaded byte must be
known and readable; a store validates the complete writable range before changing
any byte. Crossing mapping/permission boundaries stays unavailable even when
adjacent mappings exist. This `byte-addressed-memory-1` execution-environment
policy supports captured routines such as ROM `memcpy`; it is not a claim about
hardware handling, timing or concurrent atomicity. The
[RISC-V load/store specification](https://docs.riscv.org/reference/isa/v20260120/unpriv/rv32.html)
leaves misaligned ordinary accesses to the execution environment. Instruction
fetches, MMIO and atomics retain their alignment requirements. Unaligned device
accesses consume no model response and are never decomposed into byte accesses.
Normal-memory timeline events preserve the original address and width. Any
successful overlapping store, including an unaligned one, invalidates LR/SC.

Word atomics use a single-hart, program-order environment with one exact four-byte
reservation per session. LR.W replaces the reservation; SC.W checks write access
before testing it, writes only on success (status 0), and clears it after every
attempt (status 1 for reservation failure). Any successful overlapping ordinary
store or AMO invalidates it, even if the bytes are unchanged. Disjoint writes retain
it. Reservations never cross phases or implementations. LR/SC and all nine AMO.W
operations support all aq/rl combinations in this sequential environment. This is
one explicit execution model, not a concurrency/weak-memory or interruption test.
See the [ratified A extension](https://docs.riscv.org/reference/isa/v20260120/unpriv/a-st-ext.html)
for instruction and ordering definitions.

LR requires a readable known aligned word; AMOs require read/write access and a
known old word, including swap or discarded results. SC requires an aligned writable
word even when no reservation exists. Atomics operate only on mapped memory, never
the register-bank MMIO cells. Unknown data, inaccessible/misaligned addresses and
unsupported peripheral atomics stop with `memory` / `access: atomic`; missing source
registers retain `unknown-register`. RAM atomics emit no MMIO or synthetic fence
events. Their effects can be read by subsequent guest instructions; the current
selected final-RAM and normal-memory timeline relations can observe their updates.

The `static-elf/boot-data-1/entry-registers-1/byte-addressed-memory-1/phased-regions-1/physical-goals-1/stack-words-1/single-hart-atomics-1/devices-4/external-calls-2/final-memory-1/physical-calls-1/internal-timeline-1/reviewed-projections-1/reviewed-effects-1` environment maps validated ELF
segments with their permissions and ELF-defined zero-fill. A root starts with
`ra` at the return sentinel, `sp` at the stack top, the explicit ABI words in
`a0`.. and on the stack, unknown `gp`/`tp` and every other integer register at
zero, so a prologue can save callee-saved registers without an entry adapter.
Boot-initialized data
is part of the image: a writable `PROGBITS` section whose bytes the file carries
but whose address lies in a segment's zero-filled part (a ROM copies such data
at start-up) starts with those bytes. Overlapping or out-of-file boot data is
rejected. Scenario memory is
writable non-executable RAM. Each `memory` element has `lifetime` (`phase` or
`session`) and a `seed` containing `address`, `length`, optional `fill` and a byte
prefix. For example `{"lifetime":"session","seed":{"address":12288,"length":8,"fill":null,"bytes":[1,0,0,0]}}`.
Absent fill leaves newly mapped bytes unknown. Declarations cannot replace ELF
segments or overlap the stack, one another or MMIO. Redeclaration of an existing
session RAM mapping requires the identical address, length and lifetime; its
explicit prefix/fill overwrites those bytes, while unspecified bytes retain their
state. Changing a live mapping's owner/lifetime conflicts. A cold reset discards
that mapping and permits a fresh declaration. Stack fill is an explicit condition;
zero is never inferred from absent initialization.

An invocation may also list up to 128 `preload` elements, each an `address` and
known `bytes`, for example `{"address":,"bytes":[1,0,0,0]}`. After the
phase's stack and memory are mapped and before its entry, each preload writes its
bytes into the one mapped writable region that holds all of them: ELF data,
declared RAM or the stack. A preload that crosses a region's end, lies outside
every region or targets read-only memory is rejected. A preload is an input of
the phase, like a seed: it creates no guest memory or MMIO event. An empty list
is omitted from the request's encoding.

MMIO uses explicit `models` declarations. Each has a unique live `id`, caller
`applicability` conditions, `lifetime` (`phase` or `session`) and tagged `behavior`.
For example:

```json
{
  "id": "status-script",
  "applicability": "synthetic ready-on-second-read scenario",
  "lifetime": "session",
  "behavior": {"kind":"sequence-read","address":12288,"width":4,"runs":[{"value":0,"count":1},{"value":1,"count":1}]}
}
```

| Behavior kind | Declared fields and exact semantics |
| --- | --- |
| `register-bank` | `cells` with address/width/value; reads return current values, writes replace them. |
| `constant-read` | address/width/value; reads repeat the value, writes fail. |
| `sequence-read` | address/width/runs; each run has value/count and supplies that many consecutive reads. Exhaustion and writes fail. All logical responses must be consumed before closure. |
| `w1c` | address/width/initial/clear_mask/read_clear_mask; reads return the old value then clear read-clear bits; writes clear selected one bits. |
| `read-clear` | address/width/initial/clear_mask; reads return then clear selected bits; writes fail. |
| `self-clearing` | address/width/initial/store_mask/command_mask; writes replace store-mask bits, command bits clear immediately, other bits retain state. Masks cannot overlap and initial command bits must be clear. No timing is simulated. |
| `fifo` | address/width/reads/writes; independent ordered input/output transcripts. Reads consume inputs; writes must match the next output. Both lists must be consumed. This is not a loopback queue service. |
| `indexed-bank` | index_address/data_address/width/index/values; index-port accesses select/report a slot, data-port accesses read/write it. `index:null` is unknown until explicitly written; invalid indices fail. Gaps between the two ports remain unclaimed. |
| `retained-aperture` | start/length/initial over a word-aligned range of at most 16 MiB. Every naturally aligned 1-, 2- or 4-byte access in the range that no exact port of another live model claims is retained storage: a word reads `initial` until written, then its last value, and sub-word writes merge. Exact ports inside the range take precedence; misaligned accesses fail. At most 65,536 distinct words are retained per lifetime. Every access is an ordinary MMIO event, so reads of never-written words remain evidence. Apertures cannot overlap each other or memory. |
| `command-bank` | Packed 32-bit command ports over a shared bounded bank. Exact field/control bits, initial state, samples and completion-read counts are caller inputs; details below. |

`sequence-read` admits 1..4096 ordered runs, each with a nonzero `u32` count;
the logical total must fit `u32`. Adjacent equal runs are allowed, and their
explicit representation participates in identity. Application retains a run
cursor, offset and remaining logical count without expanding the input. One
read performs bounded cursor work; cancellation precedes consumption. Warm
phases retain the cursor, cold/phase closure releases it. Record validation checks
`successful reads + remaining = logical total`, rather than the number of runs.
A long finite busy script therefore stays compact without becoming an infinite
response source. Model identity version 2 includes every repeat count.

Widths are bytes (1, 2 or 4); values and masks must fit. Exact ports must be aligned,
disjoint and outside mapped code/RAM/stack. Missing ports remain inaccessible;
partial or mismatched-width accesses do not fall through to another owner. Successful
accesses emit ordinary ordered MMIO events. Model gaps retain a memory stop plus
`model` evidence with the explicit issue. Model declarations are assumptions, not
accepted hardware facts or claims about real timing/peripherals. External-call responses are separately selected in `calls` as described below.

Every instance retains its SHA-256 definition identity, including its id, conditions,
lifetime and complete ordered configuration. `model` records precede the side's
outcome and report cumulative successful reads/writes, remaining obligations, issue,
closure and status. `open` permits warm continuation; `complete` means closed with
no issue or remaining transcript; `incomplete` preserves failure or unmet closure
obligations. Returning successfully with unconsumed required values cannot MATCH.
Unused constant/register models can close complete; their zero participation is
visible and does not prove that code touched them.

Phase models close and release state after each phase. Session models retain state
through warm phases and close before the next cold phase or the end of the request. Omit a
live declaration on warm continuation; redeclaring its id conflicts, even if identical.
An expired phase model's id/ports can be assigned anew. Both implementations own
separate instances. A blocked warm phase performs no model accesses but still emits
participation/closure evidence for existing instances.

### Packed-command bank

`command-bank` declares `selector_mask`, `data_mask`, `read_command`,
`write_command`, `busy_mask`, optional `reset_command`, `ports` and `cells`.
Selector/data masks are disjoint contiguous fields of at most sixteen bits each.
Read/write words specify distinct exact fixed bits outside those fields. The
single busy bit is disjoint and clear in every issued command. No other command
bits are ignored, and read commands must carry zero input data.

Ports are strictly ordered by aligned `address`, with a maximum of 64 per bank.
Each declares an idle `initial` response (busy clear), `initial_busy_reads` and
`busy_reads` for each new command. Cells are strictly ordered by unique unshifted
`selector`, with `initial` data and `reads:null` for retained state or an explicit
sample list. An empty sample list is exhausted. At most 4096 cells and 4096 total
samples are admitted per bank. Gaps between ports remain unclaimed.

An issue selects a cell exactly. A read samples retained state or consumes one
scripted value at issue; polling never consumes another sample. A write stages
its value. Each pending command returns busy for the declared count, then one
ready read observes completion and commits a staged write to the shared bank.
Even zero busy reads requires that ready observation. Other ports see only
committed writes. Subsequent idle reads repeat the last response. Initial busy
state is an explicit completion obligation, independent of issued commands.

Ordinary overwrite of a pending command fails. An explicitly declared reset
word may abort that port's pending command and discard its staged write; reset
itself still needs completion. It never resets cells or sample cursors. A reset
word cannot also select a declared cell as an ordinary command. There is no
implicit reset opcode or hardware timing assumption.

`model.commands` is present only for these banks. It records cumulative issued,
completed, reset, aborted and scripted-read counts, plus currently pending
commands. CPU `reads`/`writes` remain separate. `remaining_reads` counts unused
samples; pending commands separately block closure. Record validation checks declaration
identity, conservation (initial pending + issued = completed + aborted + pending),
sample totals and monotonic progress backed by new port operations. These checks
validate evidence structure; they do not re-execute guest code or qualify a
peripheral model. Unknown selectors/commands, widths, exhausted samples and pending
overwrites remain explicit gaps; capacity/cancellation fails the request.

### External-call responses

An invocation's `calls` declares ABI boundary models separately from its device
`models`. IDs are unique within each category; call targets are also unique among
live call models. A call declaration contains:

```json
{
  "id": "platform-read",
  "applicability": "synthetic one-call scenario with writable output pointer",
  "lifetime": "phase",
  "binding": {"address":8192,"boundary":"unmapped","allow_tail":false},
  "argument_words": 1,
  "responses": [{
    "return_words": [0,null],
    "outputs": [{"pointer_argument":0,"byte_offset":0,"width":4,"value":42,"scope":"normal-memory"}],
    "allocation": null,
    "delay_micros": {"kind":"constant","value":5}
  }],
  "repetition": "finite"
}
```

`repetition` is `finite` or `unbounded`. A finite declaration answers exactly one
call per response, in order; unused responses leave the model incomplete and an
extra call is `exhausted-responses`. An unbounded declaration has exactly one
response without outputs or allocation and answers every call with it, so it
suits pure observations such as requested delays. Its observation reports the
call count and zero remaining responses; every call, argument and delay remains
evidence.

The binding selects one exact aligned target address in this captured address space.
`unmapped` requires that no memory/device owns its first two bytes. `captured-code`
requires a known executable ELF load mapping there and explicitly replaces execution
of its body with the response. It never claims that the body ran. Execution
validates bindings after phase memory/device installation; it does not guess names
or absent implementations. A root entry is executed as
code, even if a model binds the same address: models intercept transfers only.
Unselected targets execute captured code normally; missing bytes stop on fetch.
Ordinary x1/x5 calls may use responses. Non-return x0 transfers require `allow_tail`;
canonical returns through x1/x5 are never intercepted. `observe-call` stops before
model dispatch, so an unconsumed required response still prevents completion.

`argument_words` selects zero to 256 physical ABI words to retain at the call site:
a0–a7, then current-SP stack words. SP must be known and 16-byte aligned; stack words
must lie in the private stack. Unknown words remain explicit and block only effects
that require their value. The backend invalidates caller-saved ra/t0–t6/a0–a7 after
a modeled return, then applies exactly the two optional `return_words`. Callee-saved
registers, SP, gp and tp retain state. A normal call resumes after the transfer;
a modeled tail returns through the incoming ra. Type/variadic lowering remains the
caller's responsibility. No absent word becomes zero.

Each response may contain ordered outputs through a selected argument plus a checked
byte offset. Widths are 1/2/4 bytes and values must fit. `private-stack` requires the
session's stack; `normal-memory` permits writable stack, ELF or declared RAM, never
MMIO. All output addresses are checked before any response write. An invalid later
output cannot leave earlier output writes in an incomplete response.
Outputs require existing memory; newly allocated memory becomes available to later
calls/instructions. Modeled writes invalidate overlapping LR reservations.

`allocation` has `address`, `size_argument`, `capacity`, `lifetime`. The address must
be 16-byte aligned and the explicit low return word must equal it. The full capacity
must be fresh and disjoint from live memory, devices and call boundaries. The known
size argument selects a leading accessible zero-initialized prefix, including an
explicit zero-length allocation; exceeding capacity is a model issue. Unused capacity
remains owned but inaccessible, even for writes. Allocation lifetime is independently
`phase` or `session`; it cannot be reseeded as ordinary RAM while live. Working-memory
admission failure is a request error, not a simulated allocator response.

`delay_micros` is null, `{"kind":"constant","value":5}` or
`{"kind":"argument","word":0}`. It emits an explicit observable value without
sleeping, advancing a hardware clock or claiming timing accuracy. Model call,
argument, output, allocation and return records preserve environment evidence;
the current relation compares delay values with MMIO/fences, and excludes those
other model records. It does not infer equivalence of pointer layouts or allocators.
The first differing event index counts only this selected observable stream.

Call declarations use the same phase/session closure rules as device declarations.
Each successful response consumes exactly one entry; exhaustion, unknown required
words, bad ownership or unused responses retain explicit `call-model` issues and
cannot MATCH. Open session responses permit warm continuation; blocked phases
consume nothing. Definition identities include applicability, binding, ABI width,
ordered responses and every effect. [Record validation](#record-validation)
authenticates argument/effect/return order against the declared response as
well as participation counts and closure.

Every case is an explicit phase with a shared `reset` for both implementations
and an `entry` in each invocation. Setup and action phases can select different
entries in the same captured address space. `cold` recreates captured images and
discards all prior mutable state. `warm` retains writable ELF bytes and declared
`session` RAM independently for each implementation. The first phase must be cold.

Registers, stack and LR reservations reset each phase; models follow their declared lifetime. `phase` RAM and
stack buffers are released after observation serialization/comparison, before the
next phase. Omitted phase RAM is inaccessible on the next warm phase; redeclaring
it initializes a fresh region from its seed. Session RAM survives until a cold
reset or the end of the request. An optional case `stack_fill` byte replaces
both targets' stack `fill` for that case's phases, so one request can cover
several stack fills; the targets' explicit stack `bytes` and argument words
still take precedence. An incomplete phase blocks subsequent warm
phases on both sides, with zero steps and explicit `blocked-by-prior-phase` evidence.
A later cold phase starts an independent chain and executes normally. Earlier
incompleteness remains in the aggregate result; a completed difference does not
block later phases. All chains share one request and its work budget; a resource
failure returns an error and no records.

Each invocation declares `goal`: `{"kind":"return"}`, `reach-symbol`, or
`observe-call`. Symbol goals contain a `target` with the mapped `source` and exact
physical `symbol` identity (object, static/dynamic table kind, table section and
index). For example:

```json
{
  "kind": "observe-call",
  "target": {
    "object":{"artifact":"ELF_SHA","location":{"kind":"standalone"}},
    "table":"static","table_section":3,"index":2
  },
  "include_tail": false
}
```

`reach-symbol` uses the same target without `include_tail`. The symbol's
`object` must be an executable the target maps. The physical symbol
must be defined FUNC/NOTYPE in its executable bytes with a matching load mapping;
zero-sized symbols and aliases are valid address identities. Data, undefined,
absolute, mismatched or unavailable symbols fail the request before execution.
No name lookup, extent inference or code analysis resolves these goals.

`reached-symbol` stops at the selected PC before decoding/executing its instruction,
after verifying fetchable bytes. `observed-call` stops after a matching direct or
resolved indirect transfer, before the callee body. Ordinary calls use link register
x1/x5; `include_tail:true` also selects x0 transfers except canonical ABI returns.
The outcome records transfer PC, target and whether this was a tail. Reaching a
symbol does not prove that it is a function, and observing a call does not prove
execution of its body. Early goals do not synthesize a return value. Returning first
produces `goal-not-reached`, retaining the observed return registers; unknown control
flow and unsupported instructions retain their ordinary gaps. Warm successors may
follow a completed early goal with a fresh entry/register/stack state and retained
session RAM, never a suspended continuation.

Comparison requires the same goal kind on both sides of each phase, except that
a vendor symbol goal may pair with a replacement return goal: a prefix claim that
compares every vendor effect before the boundary with the complete replacement.
Non-return goals, and every such pair, require both relation return selectors and
call capture disabled; the relation compares their observed event prefixes,
not unexecuted bodies. Equal prefixes with unmet goals remain `INCOMPLETE`; known
prefix differences remain `DIFF`. Record validation checks that outcome kind and
dependency blocking match the declared goals.

Each comparison case supplies `relation`; a single-implementation case uses null.
For example, compare low return, ordered MMIO/fence/delay and one exact memory pair:

```json
{
  "returns":{"low":true,"high":false},
  "events":{"timeline":{"reads":false,"writes":false,"atomics":false,"branches":false},"mmio_read":true,"mmio_write":true,"fence":true,"delay":true},
  "memory":[{"vendor":0,"replacement":0}],
  "calls":false
}
```

Each invocation supplies `observe_memory`, an array such as
`[{"name":"output","address":12288,"length":32}]`. Memory pair indices select
one range on each side; lengths must match, while addresses may differ. This is an
explicit physical pairing, not an inferred ABI/layout projection. Selected ranges
must be nonempty, disjoint and uniquely named, with at most 128 selections and
1 MiB total bytes per invocation. Every selected byte is retained, including
unchanged bytes. Fixed-size `final-memory` chunks carry bytes, availability and
knownness masks; unknown/unreadable/unmapped bytes never become known zeros.
Capturing normal memory does not read a device port or consume model values.

The [verifier](../../../blobray-verification/README.md) uses the exact per-case relation.
Low and high return words are independently selectable; event channels retain their
relative order. Unselected events and memory observations remain in the result.
`difference` identifies an event index in the selected stream, a return word, or a
memory pair/byte offset with differing values. A known selected difference yields
`DIFF`; unknown selected data or unmet goals/model obligations cannot yield `MATCH`.
Memory snapshots from unfinished phases remain evidence but cannot establish a
difference between completed final states. Already observed event or returned-value
differences remain valid independently of other unknowns or unmet model obligations.
`MATCH` covers only the selected observations in the enumerated scenarios, not every
argument, path, callback or internal state. [Selected comparison contracts](../../../blobray-docs/design/contracts.md#selected-final-memory-and-comparison-relations)
define the precise completion and ownership boundaries.
`complete` means every phase reached its declared goal and all model
obligations due at closure were met, independently of the verdict. Only a `returned` outcome proves the entry returned.
A request that runs to its end returns its records, including `DIFF` or
`INCOMPLETE`; admission, resource, integrity and execution infrastructure
failures are errors.

All instruction loops share the caller's work/deadline control. Progress uses
`table` for the case ordinal and `entry` for the last PC. Each session
reserves metadata plus event/model observation capacity before allocation, and owns admitted byte
and initialization buffers for every region. Input ELF buffers are admitted
separately and released after loading. Event vectors remain charged through
comparison and serialization, then are reused or dropped with their session.
Device configuration clones and mutable banks have separate admitted owners; phase
closure releases them before the next phase. A sorted bounded exact-port index serves
accesses; capacity retained for index reuse is distinct from live model state.
No host call-stack recursion follows the analyzed program.

Requests have 1–4096 cases (`MAX_EXECUTION_CASES`), 128 RAM seeds, 128 device and 128 call declarations per invocation,
128 live models of each category, 4096 responses per call model, 256 outputs per response,
4096 exact live ports, 4096 encoded values/runs per model list, 2048 regions per session,
and 1–1,048,576 events per implementation per case (`MAX_EXECUTION_EVENTS`).
Each distinct executable is validated and loaded once per request, and every
fresh session copies its segments, so cold phases do not reload them.
`max_events` exhaustion is a resource failure with no records; events are never
silently truncated.
`max_events` is a bound, not a reservation: each session admits event capacity
into working memory as events occur, doubling up to that bound, and keeps it for
later phases.
Records arrive in stream order: per case and side, events, final memory, device
and call models and the outcome, then the comparison; coverage follows the last
case.

After the last case, `coverage` records list the code each side reached over
the whole execution, vendor records first: strictly ascending executed
instruction addresses and, for each conditional branch that executed, whether it
was taken and whether it fell through, and the distinct targets of executed
indirect calls and jumps other than returns. A side's coverage is split into
records of ascending, disjoint address ranges of at most 1,536 instructions,
512 branches and 256 indirect transfers, each holding the branches and
transfers of its own instructions; empty coverage is a single empty record. Only executable captured segments count; code run
from caller RAM does not. Coverage accumulates across cold resets and does not
depend on the selected timeline. Each side keeps one mark byte per halfword of
its executable segments in working memory. Record validation requires each
side's records in vendor-then-replacement order and rejects unordered or
overlapping records, a branch with no direction or a branch direction of an
instruction that never executed.

### Record validation

`oer_analysis_blobray_verification::validate_records` checks the records of a run against
its request, the effect contracts and layout projections the request selects and
the reported verdict and completeness, independently of the execution and
comparison code that produced them. `in_process::verify` and `in_process::vendor`
apply it to every run before returning; an inconsistency is an `Integrity` error.
It checks record order per case and side, event capacity, the physical timeline
events the invocation requested, captured call arguments against their capture
profile, final-memory chunk geometry, device and call model participation,
obligations and closure, goal and blocking outcomes, effect-contract accounting
and coverage order. It rejects a `MATCH` whose case left a selected observation
unknown, an execution or model obligation unmet, a selected read or branch
outside the selected projection, or an effect unclassified or violated. It does
not re-execute instructions: physical execution remains the executor's
authority.

### In-process verification

`oer_analysis_blobray_application::in_process::verify` executes and compares one request. The caller supplies the executables its
targets name, as `Executable` values that hash their bytes once when made and
can be reused across calls, and the effect
contracts and layout projections its relations select. Those are reviewed
outside Blobray: `effect_contract_id` and `projection_id` give the identity a relation selects them by, the
SHA-256 of their canonical JSON encoding, and `verify` rejects a selection whose
content it was not given. Records, the aggregate verdict and completeness are
returned in memory, and nothing is retained. A symbol goal resolves in the given
executable whose content is its object, to a defined code symbol of its static
symbol table. `in_process::coverage` reports the
[vendor coverage](#code-coverage-of-root-closures) of such results under
identities the caller assigns.

`in_process::vendor` executes only the vendor side of a request's cases and
returns its observations and coverage. Passed as `vendor_results` to `verify`
of a request with the same vendor side (vendor target, executables, cases'
resets, stack fills and vendor invocations, event capacity and execution
identities), it replaces executing that side: only the replacement executes,
and the records equal those of a full execution. Results of another vendor
side are rejected. When an incomplete replacement case blocks the vendor side
of the next warm case, the vendor observations depend on the replacement, and
`verify` executes the request fully; `vendor_reused` reports which happened.
The results live in memory only, for repeated comparisons within one process.

### Observation dependence

With `dependence` set to the replacement ISA's semantics, `in_process::verify`
also returns `observed`: the executed replacement instructions and those that
a compared observation depends on. Replacement sessions then record a step
log (each fetched instruction, its memory accesses and events, call-model
results, and memory the environment defined); the executor is unchanged.
After each session, a forward pass gives every step its dependencies:

- the producers of the registers it reads, and for loads and atomics of the
  memory bytes it reads; device reads and environment-defined memory are
  inputs, and a call model's result registers and output bytes come from the
  calling step;
- the conditional branch it is control dependent on: a branch controls the
  steps of its frame, including the calls they make, until its immediate
  post-dominator in the CFG recovered from the frame's function entry. A
  function over 65,536 instructions, or a branch without a path to the
  function exit, controls the rest of the frame;

Sinks are the replacement observations `oer_analysis_blobray_verification::compared_observations`
lists for each case's relation, at event granularity: the step that emitted a
compared event, the argument registers of a compared call (words beyond the
eight argument registers are not followed), the selected return words, the
last writers of compared final-memory selections and projected final-state
fields, and the returning step when
the case returned. A backward walk from the sinks marks observed steps. An unconditional transfer
(call, jump or return) offers no alternative path: every later step of its
phase runs only through it, so it is marked when any later step of its phase
is, and the walk continues from it. An
executed instruction that no sink reaches is unobserved: no compared
observation of these cases would change if its result changed. Dependence is a
necessary condition for a comparison to notice a defect, not a sufficient one;
the pointer arguments that locate a call model's outputs are not followed.

`observed` also classifies what the other executed instructions reach:
`effect` lists those any emitted event or call argument depends on, compared
or not, and `state` those the memory at a case end depends on, excluding stack
and phase-lifetime memory that ends with the phase. An unobserved instruction
in neither set changes nothing that outlives its phase.

### Image patches

`patches` of an `InProcessComparison` replace bytes of the loaded replacement
image in every replacement session: `(address, original bytes, replacement
bytes)`, at most 64 bytes each, within executable image code. A patch whose
original bytes differ from the loaded ones, or that leaves executable image
code, is invalid. No binary is rebuilt, so a patched instruction is a point
mutant: a comparison whose verdict changes kills it. The vendor side and its
reuse key are unaffected.

### ISA conformance

```console
cargo test -p oer-tool-analysis-cli --test riscv_conformance -- --ignored
```

The `riscv_conformance` test checks the RISC-V executor against the official
architectural tests, with the Sail RISC-V formal model as the reference. Its
inputs are pinned in `cli/tests/riscv_conformance/inputs.toml`: the
riscv-arch-test commit with the suites it runs and the SHA-256 of the Sail
release archive. The ignored test runs with both inputs and a clang that has
the riscv32 target and lld, given by `BLOBRAY_RISCV_ARCH_TEST`, `BLOBRAY_SAIL`
and `BLOBRAY_RISCV_CC`.

Each test of the RV32 I, M, A, B (Zba, Zbb, Zbs), C (with Zcb) and Zifencei
suites is assembled once against the target model in
`cli/tests/riscv_conformance/`. The executor runs it through in-process
verification until it returns to its sentinel, and Sail runs the same ELF
until the same address; the two signatures must be equal. A reference
signature that is only the suites' fill value fails, so a test whose body was
not assembled cannot pass.

Excluded tests carry their reason and must keep failing: `cebreak-01` needs a
machine-mode breakpoint trap, `Fencei` stores into its own code, which image
loading rejects, and the `clmul` tests need Zbc, which the ESP32-S31 does not
have. Zcmp, which the executor implements, has neither architectural
tests nor a Sail model, so this check does not cover it.

### Code coverage of root closures

`in_process::coverage` reports the vendor coverage of executions that share one
vendor target, given their records and the vendor executables; executions of
different targets are rejected.
Every distinct vendor invocation entry is a root. From each root the closure
explores executable captured code by recursive descent: conditional branches,
direct jumps and calls, `auipc`/`lui` + `jalr` pairs whose target the
immediately preceding upper immediate defines, and the observed targets of any
other executed indirect transfer. A plain jump to another defined
code symbol's start is a tail call. The closure does not enter call-model
binding addresses or goal symbols; those transfer sites are
`modeled`. Indirect transfers with neither kind of target, typically ones that
never executed, and direct transfers leaving executable captured code, are
`unresolved`; a return (the decoder's `return` flow: a `jalr` through `ra`
or `t0` at offset zero that links neither, or a returning Zcmp pop) is
neither, unless an immediately preceding `lui`/`auipc` set its register. Indirect transfers the
closure follows only to their observed targets are `followed`: targets no
execution reached, such as the other arms of a jump table, are outside the
closure, so the site itself remains a location to account for.
Undecodable instructions are `gaps`.

Each closure function lists its basic blocks reached out of all, both
directions of each conditional branch, its uncovered block leaders and branch
directions, and its modeled, unresolved, followed and gap sites; a defined code symbol at
its entry names it. Each root lists its closure functions and the distinct
blocks and directions over them. `outside` counts executed vendor instructions
that no closure decoded, such as code reached through an unresolved transfer.
A block counts as reached when its leader executed. The closure is bounded by
4,096 functions and 1,048,576 decoded instructions; exceeding either is a
resource failure.
