# HIL observer build

`oer-hil-observer` builds and prepares the HIL runner
(`oer-tool-hil-runner`). The build identity and content store are format data
of `oer-hil-runs::format::observer`; readers do not need this build library.

| Module | Owns |
| --- | --- |
| `compile` | Building the runner and reading Cargo's artifact messages |
| `resolve` | Capturing the runner's resolved dependency graph for its build script |
| `prepare` | Publishing the private executable under `target/hil/runners/<sha256>/` |

The observer input registry is
[`tools/hil/model/observer-inputs.json`](../model/observer-inputs.json).
`cargo hil` prepares the runner before launching it; `cargo hil observer`
prints the prepared executable without running a scenario. These modules
are always available; the package has no feature flags.

```console
cargo test -p oer-hil-observer
```
