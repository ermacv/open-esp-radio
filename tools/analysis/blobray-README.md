# Blobray

Blobray investigates captured RV32 ELF/archive inputs inside the calling
process and returns analysis and concrete comparison results. `cargo blobray`
runs the CLI; the typed vendor scenarios call the same operations as a
library. Inputs are given as bytes and identified by their SHA-256 content;
nothing is imported, stored or published.

Use Blobray when the missing information is in a captured binary: where a
register is accessed, which calls reach it, what data a function consumes, or
how selected observations compare with compiled Rust. Implementing the hardware
operation belongs to PAC/HAL/PHY and the driver; choosing protocol behavior
belongs to the portable protocol layer.

## Choose a task

Real input needs your own identified vendor artifacts.

| Task | Interface | Reference |
| --- | --- | --- |
| List symbols and strings | `blobray symbols`, `blobray strings` | [Symbols and strings](cli/reference/inventory-linking/README.md#symbols-and-strings) |
| Inspect inputs | `oer_analysis_blobray_application::captured::inventory` | [Inventory](cli/reference/inventory-linking/README.md#inventory) |
| Investigate code | `blobray function-records`, `blobray field-accesses`, `blobray callers`, `oer_analysis_blobray_application::library::analyze_library` | [Function analysis](cli/reference/analysis/README.md#function-analysis-contract) |
| Investigate call arguments | `blobray call-arguments` | [Call arguments](cli/reference/analysis/README.md#call-arguments) |
| Investigate registers | `blobray register-accesses` | [Library register accesses](cli/reference/registers-data/README.md#library-register-accesses) |
| Check what a change does to the hardware | `blobray mmio-diff` | [MMIO difference between builds](cli/reference/registers-data/README.md#mmio-difference-between-builds) |
| Recover tables and coefficients | `oer_analysis_blobray_application::data::export` | [Tables and coefficients](cli/reference/registers-data/README.md#captured-data-tables-and-coefficients) |
| Link an image | `oer_analysis_blobray_application::linking::{link, propose_companions}` | [Linked images](cli/reference/inventory-linking/README.md#synthetic-linked-images), [ROM companions](cli/reference/analysis/README.md#explicit-rom-companions) |
| Execute and compare | `oer_analysis_blobray_application::in_process::verify` | [Execution and comparison](cli/reference/execution/README.md#concrete-execution-and-comparison), [effect contracts](cli/reference/comparison/README.md#reviewed-effect-comparison) |

| Question | Required input | Inspect in the result | Next action |
| --- | --- | --- | --- |
| What is in this archive or ELF? | The executable's bytes | Inventory and diagnostics | Select exact symbols or ranges |
| Which code accesses this MMIO range? | Libraries and an explicit address range | Observations, masks and unresolved addresses | Review physical meaning; do not infer register width from access width |
| What values or coefficients does this code use? | An exact object and data range | Exact bytes, provenance and relocation counts | Review and record required data with provenance |
| Does this compiled Rust operation agree with the vendor case? | Both compiled implementations, concrete scenarios, models and a relation | Execution completeness, selected differences and model assumptions | Fix the implementation or review the intended relation |

## Loaded bit uses

`register-accesses` schema 11 includes signature narrowing counts and `reads` to each load observation. Source bit
positions belong to that load's access width. Each bit is in exactly one class,
with precedence `uses` > `unknown` > `preserved` > `unused`:

- `uses` names the branch, dispatch index, indirect target, memory address,
  store, call argument or return register the bit reaches. `Exact` records
  bit wiring; `May` records arithmetic, comparisons or differing mappings at
  a control-flow join. One bit can reach several sinks.
- `unknown` names where following a dependency stopped: a partial CFG edge,
  an untracked or escaping stack spill, an unsupported operation, or a work
  limit. An unrelated unknown value does not affect other loads.
- `preserved` identifies bits only stored back into the same word at their
  original positions, unchanged. A moved or inverted bit has a `Store` sink.
- `unused` contains the remaining bits proven to reach no sink or stopping
  point in the analyzed function.

Dependencies flow through instruction CFG edges to a fixed point, including
loops. Joins union sources; an exact mapping remains exact when all paths
carrying that source agree. Known stack offsets transfer byte dependencies
through stores and reloads. With an ABI and `--call-summaries`, the same pass
seeds entry argument registers to compute memoized callee contracts through
`riscv-model::calls::CallSummaries`. A first analysis records the direct/tail-call
graph without contracts. The second analyzes functions in callee-first SCC
order and supplies final records, result constants and argument dependencies.
Only bits consumed inside a callee emit `CallArgument`;
returned bits continue at their output positions in the caller's `a0`/`a1`.
Ignored arguments become unused unless they reach another sink. Summaries
include tail transfers and preserve the worse precision across the boundary.
Recursive components, unresolved/external targets, flow-incomplete callees
and exhausted summary budgets consume all argument bits with May precision.
Opaque-call and value-alternative gaps alone do not hide instruction flow;
other semantic gaps prevent narrowing through a summary. Summary stack or
unsupported-flow limits remain May consumption.
Argument sinks retain the resolved callee's address, captured defining function
and symbol name; unresolved indirect callees are `null`. Blobray does not
classify diagnostic functions. Calls clear clobbered registers and keep
callee-saved register dependencies. The expression DAG
continues to supply addresses and the selected load sites.

State sets and the work queue use admitted working memory and the caller's
work/deadline checks. A function's dependency pass has an additional cap of
`max(100000, 512 * instruction_count)` work units, so a pathological CFG can
report `work-limit` without exhausting the
whole library operation. Known sinks take precedence over limits.

`stored` projects a second domain of the same CFG engine. Each register bit
holds one tag: unknown, zero, one, or a source bit; stack bytes hold eight tags.
This pass tracks every load word and entry register. Word tags use the address
and width, so distinct loads of the same word share a tag. `stored` is present
for stores in selected ranges, including selected indexed addresses; plain
unresolved stores carry none. Joins keep equal tags and
replace differing tags with unknown. Integer operations, stack forwarding and
callee return mappings use the common interpreter. Numeric alternatives retain
only constant bits shared by all alternatives. Its function budget has the same
formula, independently of the dependency pass. On exhaustion, stores outside
the forward closure of pending transfers retain their proven cells; literal
store facts remain available inside it.

Read dependencies describe paths carrying each physical source. Stored
provenance requires one word or entry tag across all incoming paths; loads of
the same word agree independently of their instruction sites.

Grouped human output prints `reads[used=... unknown=... preserved=... unused=...]`
with sink sites, precision and limit reasons. Read selection comes only from
this dataflow; no expression observations duplicate it. Blobray reports these paths; register checker policy,
reset comparison and field-model intersections belong to `cargo registers`.
Unresolved addresses remain in the output and cannot identify a register bit.

## Two different choices

- **Research method:** static analysis, concrete scenario execution or comparison.
  Analysis is not an executed trace or an equivalence proof.
- **Output format:** `--format human` or `--format json`; this changes presentation,
  not the research method or verdict.

Related tools have different owners: `cargo registers` publishes reviewed
hardware interfaces; `cargo xtask` checks and builds repository compositions;
`cargo hil` obtains device observations. Blobray does not decide product readiness.

`cargo blobray` invokes `oer-tool-analysis-cli`; the
[operator index](cli/README.md#reference-navigation) describes that interface.
CLI and JSON use the same application operations. `--help` is available on the
top-level command and subcommands. Blobray has no TUI or code generation.

## Start an investigation

The Blobray CLI and typed vendor scenarios are members of the repository's root
Cargo workspace. They share its `Cargo.lock` and `target` directory. From the
repository root, `cargo blobray` runs `oer-tool-analysis-cli` with the `tools`
profile.

Use `cargo blobray` in the checkout: Cargo rebuilds the CLI when its Rust
inputs or tracked `chips/*/platform/chip.toml` facts change. A directly invoked
binary checks a pinned input's chip facts against those present at build time
and refuses changed or newly added profiles with a `cargo blobray` rebuild
message before resolving the artifact. Both the tools and debug profiles use
this check; even a comment change in a chip profile requires a rebuild.

The Linux linker adapter uses the shared process foundation's `command`
factory to clear unrelated operation context. Linker ownership and resource
limits remain the adapter's policy; the analysis core does not depend on
subprocess mechanisms. The crate boundary test enforces that split.

```console
cargo build --profile tools -p oer-tool-analysis-cli --bin blobray
cargo blobray --format json register-accesses --input vendor=/path/to/lib.a --range 0x60000000:0x100000
```

`--working-memory-mib`, `--timeout-secs` and `--max-work-units` bound each
command cooperatively inside its process. CLI/JSON share the
[application](blobray-application/README.md) operations. Register accesses are
observations, separate from reviewed physical declarations.

Register source publication belongs to the independent
[register tool](../registers/generate/README.md). It consumes reviewed hardware models and
policies, without loading an investigation or executing vendor code.

```console
cargo registers generate --manifest chips/esp32s31/registers/publication/registers.toml --check
```

Concrete execution supports phased RV32 integer/stack arguments, atomics, explicit
goals, device/call models and reviewed runtime interfaces/FIFO services. Comparison
selects MMIO/fence/delay, low/high return words, final normal-memory ranges,
physical or reviewed call pairs, and the internal memory/branch timeline. Explicit
accepted projections map byte fields/arrays, branch coordinates and ABI word
positions while preserving unknowns and raw physical evidence.
Unknown selected values and unmet goals/model obligations cannot MATCH. Results
remain conditional on the selected cases and explicit modeling assumptions.

[Reviewed effect contracts](cli/reference/comparison/README.md#reviewed-effect-comparison) are
implemented and retain their conditional claim ceiling. Unsupported execution
remains `INCOMPLETE`. The typed vendor scenarios run the same execution and
comparison inside their own process through
[in-process verification](cli/reference/execution/README.md#in-process-verification).

The [architecture](blobray-docs/design/architecture.md), [contracts](blobray-docs/design/contracts.md)
and [workflows](blobray-docs/design/workflows.md) describe the implemented scope. Qualifying production behavior remains an external responsibility.
