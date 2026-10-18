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
| Inspect inputs | `blobray_application::captured::inventory` | [Inventory](cli/reference/inventory-linking/README.md#inventory) |
| Investigate code | `blobray function-records`, `blobray field-accesses`, `blobray callers`, `blobray_application::library::analyze_library` | [Function analysis](cli/reference/analysis/README.md#function-analysis-contract) |
| Investigate call arguments | `blobray call-arguments` | [Call arguments](cli/reference/analysis/README.md#call-arguments) |
| Investigate registers | `blobray register-accesses` | [Library register accesses](cli/reference/registers-data/README.md#library-register-accesses) |
| Check what a change does to the hardware | `blobray mmio-diff` | [MMIO difference between builds](cli/reference/registers-data/README.md#mmio-difference-between-builds) |
| Recover tables and coefficients | `blobray_application::data::export` | [Tables and coefficients](cli/reference/registers-data/README.md#captured-data-tables-and-coefficients) |
| Link an image | `blobray_application::linking::{link, propose_companions}` | [Linked images](cli/reference/inventory-linking/README.md#synthetic-linked-images), [ROM companions](cli/reference/analysis/README.md#explicit-rom-companions) |
| Audit a final image | `blobray audit-targets` | [Target audit](cli/reference/analysis/README.md#final-image-target-audit) |
| Execute and compare | `blobray_application::in_process::verify` | [Execution and comparison](cli/reference/execution/README.md#concrete-execution-and-comparison), [effect contracts](cli/reference/comparison/README.md#reviewed-effect-comparison) |

| Question | Required input | Inspect in the result | Next action |
| --- | --- | --- | --- |
| What is in this archive or ELF? | The executable's bytes | Inventory and diagnostics | Select exact symbols or ranges |
| Which code accesses this MMIO range? | Libraries and an explicit address range | Observations, masks and unresolved addresses | Review physical meaning; do not infer register width from access width |
| What values or coefficients does this code use? | An exact object and data range | Exact bytes, provenance and relocation counts | Review and record required data with provenance |
| Does this compiled Rust operation agree with the vendor case? | Both compiled implementations, concrete scenarios, models and a relation | Execution completeness, selected differences and model assumptions | Fix the implementation or review the intended relation |

## Two different choices

- **Research method:** static analysis, concrete scenario execution or comparison.
  Analysis is not an executed trace or an equivalence proof.
- **Output format:** `--format human` or `--format json`; this changes presentation,
  not the research method or verdict.

Related tools have different owners: `cargo registers` publishes reviewed
hardware interfaces; `cargo xtask` checks and builds repository compositions;
`cargo hil` obtains device observations. Blobray does not decide product readiness.

`cargo blobray` invokes `blobray-cli`; the
[operator index](cli/README.md#reference-navigation) describes that interface.
CLI and JSON use the same application operations. `--help` is available on the
top-level command and subcommands. Blobray has no TUI or code generation.

## Start an investigation

Blobray and the typed vendor scenarios form the separate `tools/blobray` Cargo
workspace, with its own `Cargo.lock` and `tools/blobray/target` directory, so
Blobray dependencies never change the radio workspace. From the repository
root, `cargo blobray` and `--manifest-path tools/blobray/Cargo.toml` select it.

The Linux linker adapter uses the shared process foundation's `command`
factory to clear unrelated operation context. Linker ownership and resource
limits remain the adapter's policy; the analysis core does not depend on
subprocess mechanisms. The crate boundary test enforces that split.

```console
cargo build --manifest-path tools/blobray/Cargo.toml --profile blobray -p blobray-cli --bin blobray
cargo blobray --format json register-accesses --input vendor=/path/to/lib.a --range 0x60000000:0x100000
cargo blobray --format json audit-targets --artifact firmware.elf --forbid rom=0x40000000..0x40100000
```

`--working-memory-mib`, `--timeout-secs` and `--max-work-units` bound each
command cooperatively inside its process. CLI/JSON share the
[application](crates/application/README.md) operations. Register accesses are
observations, separate from reviewed physical declarations.

Register source publication belongs to the independent
[register tool](../registers/README.md). It consumes reviewed hardware models and
policies, without loading an investigation or executing vendor code.

```console
cargo registers generate --manifest registers/esp32s31/publication/registers.toml --check
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

The [architecture](docs/design/architecture.md), [contracts](docs/design/contracts.md)
and [workflows](docs/design/workflows.md) describe the implemented scope. Qualifying production behavior remains an external responsibility.
