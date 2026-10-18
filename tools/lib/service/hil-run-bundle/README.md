# HIL run bundle

`oer-tool-service-hil-run-bundle` owns the format of a HIL run bundle, its one writer and
its one reader; every producer and consumer of a run goes through it.

| Module | Owns |
| --- | --- |
| `run` | The typed documents (manifest, plan, suite, scenario and repetition results with their measurements (name, value, unit, `semantics` version, declared `better` direction, threshold, verdict; `MetricId` is their identity), the cleanup and USB records of a repetition, the integrity index and the scenario seals), the writer `RunSession` (create, bind sources and firmware, seal a scenario, finish, mark interrupted), `integrity` (the seal and its one verification, every file hashed again) and `validation` (the structural rules writer and readers share) |
| `read` | `RunBundle::open`, the typed reader: manifest, plan, suite, events, scenario documents and results, cleanup and USB records, lab and build provenance, the verified integrity seal and scenario seals, the runner's checkout, liveness |
| `store` | `RunStore`: the store every checkout shares (`$OER_HIL_STORE`, else `<XDG data>/open-esp-radio/hil`) with `runs/`, `revision-runs/` (runs another revision's runner wrote in its own bundle format, which nothing reads as store runs and no retention deletes), the observer builds, the sidecars (pins, quarantine, performance baselines, caches) and a checkout's link `target/hil/runs` |
| `receipt` | `OER_HIL_RUN_RECEIPT`, the receipt a runner names the runs it creates in, and `RunId` |
| `build` | The build provenance of a run's images and the content-addressed object store: the one definition of a build's identity, which the image builder fills; it archives the runtime ELF deflate-compressed |
| `archived` | How a bundle keeps an archived file: the runtime ELF deflate-compressed at `<path>.deflate` (recorded by its uncompressed path, size and digest), every other file as it is; `read_archived` and `archived_identity`, the one reader every consumer goes through |
| `verify` | Offline verification of whole bundles (`cargo hil report verify`) and of a replay source against the image builder's recipe |
| `experiment`, `lab` | The A/B experiment and the laboratory cell a run records |

Every document of a bundle carries a schema version (`RUN_SCHEMA`, the
scenario seal's `ATTEMPT_SEAL_SCHEMA`, `LAB_PROVENANCE_SCHEMA`,
`BUILD_PROVENANCE_SCHEMA`, `OBSERVATIONS_SCHEMA`, the source snapshot's
`MANIFEST_SCHEMA`). `format/tests/schema/run-schema-<RUN_SCHEMA>/` holds one
document of each kind with its optional parts present, the fixture records of
`run::fixtures` included (`helper.json` wraps a family's own type and is the
family's to keep readable), and
`format/tests/schema.rs` reads each back unchanged: a change a stored bundle
would not survive (a new required field, a renamed, removed or retyped one)
fails there and needs a schema bump, with those documents moved to the new
schema's directory and brought to the new shape. A new optional field that
defaults when absent needs none.

The JUnit and HTML views a run seals are rendered by `oer-tool-service-hil-analysis`,
which the runner hands `RunSession::finish`. The bundle depends on no
stand, board or image builder code.

```console
cargo test -p oer-tool-service-hil-run-bundle
```
