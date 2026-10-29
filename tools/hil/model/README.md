# HIL catalog and evidence model

`oer-hil-model` defines the shared values read by the executable scenario
catalog, runner and bundle readers. Execution and workload contracts live in
[`oer-hil-scenario`](../scenario/src/lib.rs); run orchestration lives in
`tools/hil/run`.

| Module | Owns |
| --- | --- |
| `catalog` | Scenario headers, catalog discovery, profile requests, laboratory requirements and Wi-Fi link vocabulary |
| `schema` | Canonical scenario snapshots, run evidence and image classes with their feature deltas |
| `image_class` | Chip image recipes and build features derived from the current profiles and firmware manifests |
| `health` | Radio-health snapshots and differences used by host workloads |

```console
cargo test -p oer-hil-model
```
