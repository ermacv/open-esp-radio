# HIL run analysis

`oer-hil-analysis` reads run bundles through `oer_hil_run_bundle::RunBundle`
and never decides qualification.

| Module | Owns |
| --- | --- |
| `run` | `Run`: a bundle with its suite, its status and whether its runner is gone |
| `samples` | The one aggregation of a run's typed measurements (`samples`, one value per repetition and a gate from the threshold), the one comparison of two sets of values (`compare`: Welch's 95 % interval and a 2 % practical tolerance) and the one judgement against a baseline (`change`) |
| `runs` | `cargo hil runs` queries: list, show, why, compare (the noise-aware comparison of both runs' repetitions), history, stability, flaky; following a run to its end (`wait`); collecting unnamed observer builds |
| `retention` | The prune rule, the store's size budget and `prune` |
| `perf` | Gated measurements per commit and layout, reviewed baselines in the store's `perf-baselines.json`, regressions, cached summaries |
| `arms` | The comparison of an A/B experiment's arms: one value per run, the mean of its repetitions |
| `dashboard` | The runs of the stand's live page and a running run's progress |
| `report` | The JUnit and HTML views a run seals beside its suite |

```console
cargo test -p oer-hil-analysis
```
