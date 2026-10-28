# Verification harness

`oer-vendor-scenario` holds the vendor verification: the chip-neutral engine,
the ESP32-S31 scenarios on it and the reviewer reports:

| Package | Path | Owns |
| --- | --- | --- |
| `oer-probe-codegen` | `tools/probe/codegen/` | Rust declaration grammar, image catalog, linker roots and post-link validation |
| `oer-probe-macros` | `tools/probe/macros/` | Procedural-macro frontend of the declarations |
| `oer-vendor-scenario` | `tools/vendor/scenario/` | The engine at the crate root: authenticated artifacts, Blobray sessions, comparison harness, observation and state analysis; `esp32s31`: the chip's scenarios, addresses, pins and reviewed decisions; `report`: unclaimed compared pairs, the shared scenario command line and the `xref`, `show`, `fields` and `prints` inspection commands |

The entry package `oer-tool-vendor-scenario-cli` (`tools/vendor/scenario-cli`)
installs the ESP32-S31 scenarios and runs one scenario or inspection command.
`cargo vendor-check scenario --chip esp32s31` builds and runs it; a chip
without a module below `tools/vendor/scenario/src/` has no scenarios.

Observation decisions name the file and trimmed source text ending at the
reviewed line. Include preceding lines when an identical statement occurs in
different functions; only the final line is reviewed. `Sources::check` rejects
an anchor whose line is now observed or unexecuted, including when only an
identical statement in another context remains unobserved.

The probe compiler is a host build tool; it owns no production behavior and
does not depend on Blobray's executor. The
[ESP32-S31](../../../firmware/probes/esp32s31.md) probe packages consume it. A chip's
scenario module installs its `Chip` description into the engine and keeps its
addresses, pins and reviewed decisions; the
[ESP32-S31 probes](../../../firmware/probes/esp32s31.md) describe the inputs.

The engine decides every verdict and hands each claimed suite's `Findings`
to a `RunReport` supplied by the scenario binary. The report crate depends
on the engine. Each suite writes a schema-1 `<suite>-report.request.json`;
the reviewer prints its resource document. `analysis_resources` lists each
successful inventory, catalog, data, link and execution operation with the shared
`AnalysisResources` shape. The `vendor-scenario` defaults are 256 MiB,
2,000,000,000 work units and 600 seconds; CLI budget flags override them and the
report records the actual limits and usage. Executions have no function-local
widening outcomes; operation exhaustion remains an error without a successful
resource result. Failure JSON is schema 1 and retains the completed operations'
resources. Repeated operations are separate, so recurring function identities
are never added as if they were disjoint. Standalone inspection commands use
their own interpreter and do not run these budgeted comparison operations.

`cargo vendor-check copies --chip <chip>` writes schema-3
`target/verification/<chip>/copies.json`. Its `analysis_resources` lists the
`vendor-copies` resources and input identities of each MMIO comparison or gap
inventory, keeping repeated ROM comparisons separate. Defaults are 8192 MiB,
1,000,000,000,000 work units and 1200 seconds; budget flags override them.
Typed counts come directly from each `mmio_diff` result; operation exhaustion
fails without writing a report for that run.

Pins come from `oer-tool-pins`, the reader of `artifacts.toml`.

## Declare an entry

Place a declaration in an ordinary Rust module of the probe library:

```rust,ignore
oer_probe_macros::probe! {
    /// Run the shipping operation under the selected isolated owner.
    pub fn open_example(value: u32) -> u32 =>
        with_owner(|owner| shipping_operation(owner, value).map_or(0, project));
}
```

The signature declares the guest ABI. The expression explicitly chooses argument
conversion, setup, production call and result projection. The compiler generates
`extern "C"`, `no_mangle` and `inline(never)`. An ordinary function body is also
accepted for adapters with control flow. `#[unsafe(naked)]` retains an explicit
assembly entry without adding an incompatible inline attribute. Safety and
ownership obligations remain beside the declaration; this syntax does not make
an invalid guest pointer or hardware state valid.

Generic, async, variadic, conditional and explicitly ABI-qualified declarations
are rejected. Drive an async production operation inside a synchronous adapter.
Use named parameters. Modules containing declarations must be ordinary inline
or file modules; conditional modules other than host `cfg(test)` are rejected.
Do not hide probes inside another macro or generated include: collection visits
source module items, not arbitrary macro expansion. An unregistered `open_*`
C function is an error. Names are unique within an image; separate images may
publish the same name.

## Build and inspect

Each ELF build script calls the shared collector with its library source root,
package identity and runtime `OUT_DIR`. Every input module is registered with
Cargo change tracking. Roots are generated as individual `--undefined` arguments;
the binary explicitly links its probe library. No runtime retention function or
hand-maintained symbol list participates in linking.

The same declarations generate a schema-1 UTF-8 JSON catalog in `.blobray.probes`.
The ELF includes the generated byte array, and its linker script retains that
section. Entries contain the exact symbol, C signature, ordered named Rust
boundary types and expression/body adapter kind. Rust type spelling describes
the boundary; it does not establish the layout of a private production type.
Addresses are resolved from the final ELF, not predicted during compilation.

`cargo vendor-check probes --chip esp32s31` builds every declared probe image
and validates every catalog against defined executable symbols. Missing,
ambiguous, non-code, malformed or wrong-image entries fail the build command.
Link-time code folding may give different entry names the same address.
Plain Cargo compilation produces an image; the vendor-check command additionally
establishes its catalog/entry consistency before it is used as verification input.

Generated Rust, catalogs and inspection outputs stay in ignored build storage.
The catalog is captured inside the ELF, so later readers need no source checkout
or sidecar file.

## Validation

```console
cargo test -p oer-probe-codegen -p oer-probe-macros
cargo test -p oer-probe-codegen --test linking -- --ignored
cargo test -p oer-vendor-scenario
```

The second command requires the repository's RV32 target. It tests archive
extraction with fat LTO and section GC, then adds an entry and relocates the
source directory. Ordinary host tests cover adapters, collection errors and
malformed/missing/non-executable ELF entries.
