# HIL radio families

Each directory is one radio family's crate, `oer-hil-family-<family>`. A
family owns one scenario table (`[system]` in a scenario document) and
everything that interprets it; the runner core names no family type.

| Crate | Owns |
| --- | --- |
| `ieee80211-fixture/` | The Wi-Fi fixtures (local Linux and OpenWrt access points, hostapd, host routes, air monitors), their `FixtureProvider`, and the air-capture and BSS-protection analyses |
| `system/` | The `[system]` table: boot smoke, watchdogs, panic reset and stand recovery |
| `phy/`, `phy-esp32s31/` | Not a scenario family: the PHY cross-check's comparison port, its ESP32-S31 implementation and the vendor reference firmware driver |

## The family contract

A family's table implements `oer_hil_scenario::ScenarioFamily` (validation,
plan, requirements, the image keys that serve it, the peer image it needs
and the air it occupies) and `oer_hil_workload::family::Workload` (its
precondition and `run`), and the crate exports `pub const FAMILY: Kind`. The
runner lists every `FAMILY` and fixture provider once, in its `Registry`
(`hil/host/runner/src/scenario.rs`), and dispatches through it.

A workload receives the repetition `Context` and the prepared `Fixtures`. It
reports through the context: typed values with `context.results.observe`,
its claim with `context.results.claim` and measurements with
`context.measurements`; `Context::finish` writes them once as the
repetition's `observations.json`. Per-boot work goes through
`oer_hil_workload::boots::for_each_boot`, and a workload that needs image keys
asks `oer_hil_workload::keys::require_keys`. It talks to the target with the
link's generic `call`, `request` and `command`; a family keeps its exchanges
in its own module, never in `oer-hil-link`. A workload that must write
another image onto the board under test asks `Context::images` for the run's
`BoardImages` (the flash operation under the run's lock); it never flashes
on its own.

A family depends on `oer-hil-workload`, the link, the scenario envelope and
the network traffic crate, never on another family. A fixture provider implements
`oer_hil_workload::family::FixtureProvider` and inserts what it prepares into
`Fixtures`, where a workload finds it by type.

## Add a family

1. A crate `family/<family>/` named `oer-hil-family-<family>`, `layer =
   "hil"`, `hil = "orchestration"`, with its table, `Workload` and `FAMILY`.
2. Its scenarios under `hil/scenarios/<family>/`.
3. Its `FAMILY` in the runner's registry and its crate in the runner's
   dependencies and in its scope of `hil/schema/observer-inputs.json` (the
   observer's workload identity). A family may reach a crate of an excluded
   workspace; the observer resolves its inherited dependencies through that
   workspace's manifest.

```console
cargo test -p oer-hil-family-system \
  -p oer-hil-family-ieee80211-fixture -p oer-hil-family-phy \
  -p oer-hil-family-phy-esp32s31
```
