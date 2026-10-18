# HIL observer identity

`oer-hil-observer` owns the build identity of the HIL observer, the runner
(`oer-hil-runner`), which the runner records in every run.

| Module | Owns |
| --- | --- |
| `inputs` | The observer input registry ([`hil/schema/observer-inputs.json`](../schema/observer-inputs.json)) and the projection of the runner's resolved graph onto each workload's dependencies |
| `cargo_inputs` | The local package closure of a captured workspace and lock |
| `artifacts` | Cargo's actual compilation units applied to the resolved graph |
| `store` | The content store of observer builds (`observers/<sha256>.json` beside the runs, in a run store or an evidence archive copy) and the reference form a run's record takes |
| `receipt` | The receipt of a prepared runner, `OER_OBSERVER_RECEIPT` and the checkout's current descriptor `target/hil/current-observer.json` |
| `compile`, `resolve`, `prepare` (feature `producer`) | Building the runner from Cargo's artifact messages, resolving its graph (the runner's build script), and its private copy under `target/hil/runners/<sha256>/` with the invocation's receipt (`prepare`), or without reading its embedded build or writing a receipt (`build`, for another revision's runner, whose build record is in that revision's format) |

`cargo hil` and `cargo hil observer` prepare the observer.

```console
cargo test -p oer-hil-observer --features producer
```
