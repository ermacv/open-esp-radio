# Blobray application

`blobray-application` owns the operations Blobray performs over executables
its caller gives as bytes, inside the caller's process. Its internal
dependencies are [domain](../domain/README.md),
[artifacts](../artifacts/README.md), [analysis](../analysis/README.md) and
[verification](../verification/README.md). It does not depend on a concrete
ISA backend or linker adapter: both are injected.

Every operation takes `in_process::Executable` values, a `WorkingMemory` and a
`RunControl`, and returns or streams its result; nothing is retained after it
returns. A request names executables by content, and `in_process::find` fails
with `invalid-request` when it names content that was not given.
`in_process::Limits` is the work-unit, deadline and run-position control the
CLI and the vendor scenarios use. See
[cooperative control](../../cli/reference/resources/README.md#cooperative-control-and-failure-diagnostics)
for policy and working-memory boundaries.

## Captured executables

`captured` enumerates an executable's container members in order and inspects
each object once: `inventory` returns the `ArtifactInventory`, and the other
operations visit members through the same cursor. A thin archive member and a
malformed member payload are members with a diagnostic; broken framing ends
enumeration and marks the membership incomplete. See
[inventory](../../cli/reference/inventory-linking/README.md#inventory).

## Library analysis and register accesses

`library::analyze_library` selects every function symbol of an executable
section in every object, prepares each object once, and runs the analysis
engine with an injected `FunctionSemantics` for each function. Missing extents
and unsupported functions become blocked outcomes; code no function covers
becomes a gap; resource and integrity failures fail the call.
`library::register_accesses` filters the analyzed memory accesses by optional
ranges and streams `RegisterAccess` records with a summary. Observed access
widths and masks are never promoted to hardware declarations; source
publication remains with the independent register tool. See
[function analysis](../../cli/reference/analysis/README.md#function-analysis-contract).

## Linking and ROM companions

`linking` owns the synthetic image policy and the `LinkerHost` port. `link`
materializes the request's selected objects in a private `LinkWorkspace`, passes
layout, exact ordered members, roots and companion definitions to an identified
`static-analysis-elf-link-v1` adapter, and validates every linker claim:
normalized placement and extraction evidence must prove the exact entry and root
occurrences, and the ELF must pass structural validation. `LinkOutputSink`
receives raw output, normalized observations and the reaped seekable ELF.
Application never parses linker-specific map text. `companions` validates exact
ROM function and data definitions; `proposal` trial-links a request and proposes
them from ordered candidate executables. See
[linked images](../../cli/reference/inventory-linking/README.md#synthetic-linked-images)
and [ROM companions](../../cli/reference/analysis/README.md#explicit-rom-companions).

## Data and audit

`data::export` resolves one exact object and borrows every selected range from
one prepared object; it returns the bytes with per-span provenance and
relocation counts and never applies a relocation. `audit::audit_targets`
validates a final image's executable sections through the artifact owner and
streams forbidden-target findings and coverage gaps with a summary. See
[captured data](../../cli/reference/registers-data/README.md#captured-data-tables-and-coefficients)
and [target audit](../../cli/reference/analysis/README.md#final-image-target-audit).

## Concrete execution

Concrete execution runs only inside the calling process, over caller-supplied
executable bytes. `in_process` exposes it:
`verify` for a request, `vendor` for its vendor side alone, whose results a
later `verify` of the same vendor side reuses, and `coverage` over the results.
`execution` loads the executables and runs a resolved request;
`execution_memory` owns mutable session regions, initialization state and
bounded events. `Executor` is injected from domain; application has no concrete
ISA dependency. The pure verification crate owns comparison and the
independent record validation `verify` and `vendor` apply to every run. See
[execution and comparison](../../cli/reference/execution/README.md#concrete-execution-and-comparison)
for stateful lifetimes, resource obligations and claim limits.

`execution_coverage` accumulates the code one side reaches over all its
sessions; `code_coverage` compares that with the static closure of the vendor
roots for `in_process::coverage`. With
dependence requested, `execution_steps` records each replacement session's
step log and `dependence` derives which executed instructions the compared
observations depend on. Image patches replace replacement bytes in every
session without a rebuild. See
[in-process verification](../../cli/reference/execution/README.md#in-process-verification).

Concrete invocation setup owns the aligned stack argument area and its knownness.
Explicit unknown words invalidate seeded bytes before execution. Domain validates
physical ABI word capacity/placement; the backend receives optional a0–a7 words.
All setup and phase resets share the execution budget and request.

Each concrete session owns one exact four-byte LR reservation. SC always clears
it and checks write permissions; overlapping successful writes invalidate it.
AMO updates validate/admit before invoking backend arithmetic once. The environment
executes one hart in program order; reservations reset between phases and atomic
MMIO is unsupported.

Execution cold phases recreate captured mappings; warm phases retain writable ELF
and session RAM. Phase RAM and stack release after comparison.
An incomplete phase blocks warm successors; a cold phase starts another independent
chain within the same budget. Live RAM lifetime changes conflict.

All execution goals resolve in the executable their symbol names by content
before sessions are allocated. Early goals close
a phase without claiming return or callee-body execution; warm successors start
their own entry with only session-owned memory retained.

`devices` owns admitted configuration/state, exact sorted ports and phase/session closure. Session snapshots model participation before releasing closed instances; verification sees both code outcomes and due model obligations. Warm continuation cannot redeclare a live id. All models use the shared execution/replay path and budget.

`external_calls` owns immutable response copies, admitted instances and cursors. `execution_memory::execution_calls` validates complete effects, writes only checked normal memory, and owns bounded allocations. Phase/session closure and call evidence share the execution budget and capacity.

`execution_memory::execution_observation` admits and captures exact selected normal
memory at the phase stop before releasing phase owners. It preserves unchanged,
unknown and unavailable bytes in bounded chunks, without device reads. Snapshot
capacity lives through comparison and is released on recycle. Each
comparison case supplies its own relation to verification; no frontend composes it.

Effect contracts and layout projections are reviewed outside Blobray and reach
a comparison by content through `in_process::verify`, which validates each
selected contract against the request.
