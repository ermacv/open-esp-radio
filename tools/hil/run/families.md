# HIL radio families

The registry in `tools/hil/run/src/scenario.rs` composes four families.
Each owns a scenario table and its executable workload.

| Package path | Table and workload |
| --- | --- |
| `tools/hil/family-system/` | `[system]`: boot smoke, watchdog and panic reset |
| `tools/hil/family-lower-mac/` | `[lower-mac]`: conformance and ownership-close checks |
| `tools/hil/family-wifi/` | `[wifi]`: W4 qualification and the initial-GTK investigation |
| `tools/vendor/phy-esp32s31/` | `[phy]`: calibration comparison and tracking investigations, composed over `tools/vendor/phy/` |

## Family contract

A family's table implements `oer_hil_scenario::ScenarioFamily` (validation,
plan, requirements, the image keys it requires, peer image and air use) and
`oer_hil_scenario::workload::family::Workload` (precondition and execution).
The runner refuses a flashed image whose reported keys lack a required one.
It exports a `FAMILY: Kind`, listed once in the runner's `Families` registry.
The workload receives its repetition `Context` and `Fixtures`.

A workload reports typed observations and claims through `context.results`
and measurements through `context.measurements`. `Context::finish` writes
`observations.json`. Per-boot work uses
`oer_hil_scenario::workload::boots::for_each_boot`; required image keys use
`oer_hil_scenario::workload::require_keys`. Protocol exchanges belong to the
family and use the generic `oer-hil-link` API. A workload writing a different
image uses `Context::images`, the run's leased `BoardImages` port.

Fixture providers implement
`oer_hil_scenario::workload::family::FixtureProvider`; the current runner's
provider registry is empty. The Wi-Fi and lower-MAC families use their own
managed-router operations. Scenario documents live under
`tools/hil/scenarios/`; the observer input registry is
`tools/hil/model/observer-inputs.json`.

```console
cargo test -p oer-hil-family-system -p oer-hil-family-lower-mac \
  -p oer-hil-family-wifi -p oer-vendor-phy -p oer-vendor-phy-esp32s31
```
