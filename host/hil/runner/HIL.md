# Hardware-in-the-loop infrastructure

HIL runs compiled production code on an attached device and records typed,
immutable evidence bundles. It does not implement radio behavior, and a passed
scenario or internally consistent bundle is not by itself a qualification
decision.

## Choose a route

| Task | Start here | Hardware or host effects |
| --- | --- | --- |
| Understand execution, cleanup and the run bundle | [Execution and evidence architecture](host/architecture.md) | Reading only |
| Inspect scenario definitions | [Scenario catalog](scenarios/README.md) | Reading only |
| Configure the lab and run commands | [Host setup and operations](host/README.md) | The guide labels build, fixture and DUT effects |
| Share the stand: leases, boards, firmware catalog | [Sharing the stand](host/stand.md) | Leases, flashes and resets of named boards |
| Find, compare and investigate runs | [Runs and investigation](host/runs.md) | Reading only, except enqueued runs, bisection, revision runs and A/B comparisons, which run scenarios |
| Preview or install Linux fixture software | [Canonical fixture installation](host/fixtures.md#linux-fixture-software-installation) | Dry-run is offline; installation uses interactive privilege but no RF |
| Build or understand target firmware | [ESP32-S31 target](targets/esp32s31/README.md) | Build-only sections are separate from flash/run sections |
| Install or operate the Linux network fixture | [`linux-net`](host/linux-net/) and its checked helper interface | Requires explicit host setup and, for installation, privileges |

The typed scenario model that replaces the scenario documents lives in
[`model/`](model/README.md), its execution in
[`host/executor/`](host/executor/README.md) ().
The wire contract lives in `protocol/`, versioned scenarios in `scenarios/`,
host orchestration in `host/runner/`, and embedded consumers in `targets/`.
Chip-independent target logic and its host tests live in
[`agent/`](agent/README.md).
The evidence contract — the run vocabulary and canonical scenarios — lives
in the `schema/` crate; the observer's build identity and its content store
live in [`observer/`](observer/README.md), whose `producer` feature
adds the Cargo-running operations that only the runner and repository tools
use. The run bundle's format, writer and its one reader are
[`host/run-bundle/`](host/run-bundle/README.md).
The PHY calibration cross-check libraries live in `phy/`: the chip-neutral
[`capture/`](phy/capture/README.md), and each chip's shared relation and
comparison, for the ESP32-S31
[`relation/`](phy/esp32s31/relation/README.md) and
[`calibration/`](phy/esp32s31/calibration/README.md).
They belong to the root workspace; only `targets/<chip>/` builds
in separate firmware workspaces.
Generated runs stay below `target/hil/runs`; they are not tracked.
A run never writes tracked files.

## Safe source-only route

From the repository root, these commands do not open a serial device, change a
fixture, flash a DUT, or transmit RF:

```console
cargo hil scenario list
cargo hil scenario validate
```

`cargo hil image build <image-class>`
is build-only, but it executes the embedded build/tooling pipeline and writes
artifacts. `doctor` inspects machine and fixture prerequisites; it is not a
source-only check and does not prove that the current fixture configuration is
ready for a scenario.

## Hardware route

Copy `host/lab/model/stand.example.toml` to the mode-0600 stand file
`~/.config/open-esp-radio/stand.toml`, shared by every checkout, describe
the stand's hubs and boards and the required fixture values, install only the
helpers required by the selected scenario, then run `cargo hil doctor` and
`cargo hil plan <scenario-id>`. Flash, replay, fixture-check and run commands
can reset or modify the named DUT/fixture and require their exclusive leases;
follow the [host guide](host/README.md) for their exact prerequisites and
effects. “Without a DUT” does not mean “without host or peer-adapter changes.”

## Run and evidence lifecycle

The runner resolves the catalog plan and builds the current-source images into
the new run directory. It then waits for the stand's lease from the
[arbiter](../stand/arbiter/README.md) on the boards, fixtures and air it claims,
acquires cooperative serial/fixture leases and captures secret-free lab provenance while the fixture lock is held.
It imports replayed firmware and flashes the archived `application.bin`, not an
unrecorded build output. Each repetition
owns its process, UART and fixture resources; partial observations survive
ordinary errors and cancellation.

Cleanup runs before repetition attachments and its result is stored separately
from the workload failure. A cleanup failure can make the repetition broken
and quarantine later network work; it cannot turn the workload into a pass.
Each scenario's results are written into its directory as they finish, so
they remain if a later scenario or the campaign is interrupted. After scenario
results are written, the runner writes suite/JUnit/HTML records and finally
marks the manifest complete. Interrupted unwinding instead records an
interrupted manifest. A run carries no seal: a bundle is the files its writer
left, and a run of another `RUN_SCHEMA` is not read, but stays in the store.

History pages and Markdown remain navigation and presentation, never proof
input.
