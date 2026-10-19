# HIL observer identity

`oer-hil-observer` owns the build identity of the HIL observer, the runner
(`oer-hil-runner`), which the runner records in every run.

| Module | Owns |
| --- | --- |
| `inputs` | The observer input registry ([`hil/schema/observer-inputs.json`](../schema/observer-inputs.json)) and the projection of the runner's resolved graph onto each workload's dependencies |
| `cargo_inputs` | The local package closure of a captured workspace and lock |
| `store` | The content store of observer builds (`observers/<sha256>.json` beside the runs of a run store) and the reference form a run's record takes |
| `compile`, `resolve`, `prepare` (feature `producer`) | Building the runner from Cargo's artifact messages, resolving its graph (the runner's build script), and its private copy under `target/hil/runners/<sha256>/` (`prepare::build`) |

`cargo hil` builds the observer; `cargo hil observer` builds it alone and prints its private copy.

```console
cargo test -p oer-hil-observer --features producer
```
