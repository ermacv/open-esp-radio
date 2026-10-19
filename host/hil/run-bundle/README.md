# HIL run bundle

`oer-hil-run-bundle` owns the format of a HIL run bundle, its one writer and
its one reader; every producer and consumer of a run goes through it.

| Module | Owns |
| --- | --- |
| `run` | The typed documents (manifest, plan, suite, scenario and repetition results with their measurements (name, value, unit, `semantics` version, declared `better` direction, threshold, verdict; `MetricId` is their identity), the cleanup and USB records of a repetition), the writer `RunSession` (create, bind firmware, finish, mark interrupted), `files` (a scenario's attachments and a directory's inventory) and `validation` (the structural rules writer and readers share) |
| `read` | `RunBundle::open`, the typed reader: manifest, plan, suite, events, scenario documents and results, cleanup and USB records, lab provenance, the runner's checkout, liveness; a run of another `RUN_SCHEMA` is an error naming both schemas, and the run stays as it is |
| `store` | `RunStore`: the store every checkout shares (`$OER_HIL_STORE`, else `<XDG data>/open-esp-radio/hil`) with `runs/`, `revision-runs/` (runs another revision's runner wrote in its own bundle format, which nothing reads as store runs and no retention deletes), the observer builds, the sidecars (pins, quarantine, performance baselines, caches) and a checkout's link `target/hil/runs` |
| `launch_runs` | `OER_HIL_LAUNCH_RUNS`, the list a runner names the runs it creates in, and `RunId` |
| `build` | The content-addressed object store a run's images are archived in (the runtime ELF deflate-compressed), and `repository`: the commit of a tree and whether it is dirty, which a run and each of its images record |
| `archived` | How a bundle keeps an archived file: the runtime ELF deflate-compressed at `<path>.deflate` (recorded by its uncompressed path, size and digest), every other file as it is; `read_archived` and `archived_identity`, the one reader every consumer goes through |
| `replay` | The archived firmware of an earlier run, its manifest, laboratory provenance and every firmware file checked against its recorded digest before a run replays it |
| `experiment`, `lab` | The A/B experiment and the laboratory cell a run records |

Every document of a bundle carries a schema version (`RUN_SCHEMA`, `LAB_PROVENANCE_SCHEMA`,
`OBSERVATIONS_SCHEMA`). `format/tests/schema/run-schema-<RUN_SCHEMA>/` holds one
document of each kind with its optional parts present, the fixture records of
`run::fixtures` included (`helper.json` wraps a family's own type and is the
family's to keep readable), and
`format/tests/schema.rs` reads each back unchanged: a change a stored bundle
would not survive (a new required field, a renamed, removed or retyped one)
fails there and needs a schema bump, with those documents moved to the new
schema's directory and brought to the new shape. A new optional field that
defaults when absent needs none.

The JUnit and HTML views a run writes are rendered by `oer-hil-analysis`,
which the runner hands `RunSession::finish`. The bundle depends on no
stand, board or image builder code.

```console
cargo test -p oer-hil-run-bundle
```
