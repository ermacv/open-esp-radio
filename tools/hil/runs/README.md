# HIL run bundle

`oer-hil-runs` owns the format of a HIL run bundle, its one writer and
its one reader; every producer and consumer of a run goes through it.

| Module | Owns |
| --- | --- |
| `format::run`, `run` | The typed documents (manifest, plan, suite, scenario and repetition results with their measurements (name, value, unit, `semantics` version, declared `better` direction, threshold, verdict; `MetricId` is their identity), the cleanup and USB records of a repetition), the writer `RunSession` (create, bind firmware, finish, mark interrupted), `files` (a scenario's attachments and a directory's inventory) and `validation` (the structural rules writer and readers share) |
| `format::read` | `RunBundle::open`, the typed reader: manifest, plan, suite, events, scenario documents and results, cleanup and USB records, lab provenance, the runner's checkout, liveness; a run of another `RUN_SCHEMA` is an error naming both schemas, and the run stays as it is |
| `store` | `RunStore`: the store every checkout shares (`$OER_HIL_STORE`, else `<XDG data>/open-esp-radio/hil`) with `runs/`, the observer builds, the sidecars (pins, quarantine, performance baselines, caches) and a checkout's link `target/hil/runs` |
| `launch_runs` | `OER_HIL_LAUNCH_RUNS`, the list a runner names the runs it creates in, and `RunId` |
| `build` | The content-addressed object store a run's images are archived in (the runtime ELF deflate-compressed), and `repository`: the commit of a tree and whether it is dirty, which a run and each of its images record |
| `format::archived` | How a bundle keeps an archived file: the runtime ELF deflate-compressed at `<path>.deflate` (recorded by its uncompressed path, size and digest), every other file as it is; `read_archived` and `archived_identity`, the one reader every consumer goes through |
| `replay` | The archived firmware of an earlier run, its manifest, laboratory provenance and every firmware file checked against its recorded digest before a run replays it |
| `format::experiment`, `format::lab` | The A/B experiment and the laboratory cell a run records |

The observer identity and content-store modules are in `format::observer`
(`inputs`, `cargo_inputs` and `store`). The runner producer operations live
in `oer-hil-observer`.

Every document of a bundle carries a schema version (`RUN_SCHEMA`, `LAB_PROVENANCE_SCHEMA`,
`OBSERVATIONS_SCHEMA`). A change a stored bundle would not survive (a new
required field, a renamed, removed or retyped one) needs a schema bump. A new
optional field that defaults when absent needs none.

The JUnit and HTML views a run writes are rendered by `oer-hil-stats`,
which the runner hands `RunSession::finish`. The bundle depends on no
stand, board or image builder code.

```console
cargo test -p oer-hil-runs
```
