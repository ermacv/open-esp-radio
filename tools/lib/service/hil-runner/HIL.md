# Hardware-in-the-loop infrastructure

HIL runs compiled production code on an attached device and records typed,
immutable evidence bundles. It does not implement radio behavior, and a passed
scenario or internally consistent bundle is not by itself a qualification
decision.

## Choose a route

| Task | Start here | Hardware or host effects |
| --- | --- | --- |
| Understand execution, cleanup and bundle integrity | [Execution and evidence architecture](host/architecture.md) | Reading only |
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
The evidence contract — the run vocabulary, canonical scenarios and
source-snapshot manifests — lives
in the `schema/` crate; the observer's build identity, its content store and
receipts live in [`observer/`](observer/README.md), whose `producer` feature
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
A run never writes tracked files. HIL evidence is the runs a qualification
decision cites, kept outside the repository in the evidence archive
([#761](https://github.com/the predecessor repository/issues/761) Q7): `cargo
hil archive ref RUN...` prints the runs' references (ID and the SHA-256 of
the seal) as a references file and pins the runs for a new selection named in
it, `pack --refs FILE --out DIR` writes an immutable version of exactly the
cited runs, the pinned version's from its downloaded, verified copy and the
others from the run store, and hands the selection's pins to it; `holders`
lists the selections and unpublished versions that pin runs, `unpin
VERSION|SELECTION` drops the pins of a version packed but never published or
of a selection, and `publish DIR` releases it in the pinned version's
repository (`--repository OWNER/NAME` before the first version, with
`--new-repository` to move the archive) and writes the pin `evidence/archive.toml` in `qualification/`, and `fetch --out DIR`
downloads the pinned version and verifies it. `pack`, `publish` and `fetch` reach the network; a
version is never changed or deleted. See [the archive](host/evidence-archive/README.md).

## Safe source-only route

From the repository root, these commands do not open a serial device, change a
fixture, flash a DUT, or transmit RF:

```console
cargo hil scenario list
cargo hil scenario validate
cargo hil report verify [run-id]
```

The final command reads existing local evidence and can fail when no requested
bundle exists or a seal is inconsistent. `cargo hil image build <image-class>`
is build-only, but it executes the embedded build/tooling pipeline and writes
artifacts. `doctor` inspects machine and fixture prerequisites; it is not a
source-only check and does not prove that the current fixture configuration is
ready for a scenario.

## Hardware route

Copy `stand/stand.example.toml` to the mode-0600 stand file
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
Each completed scenario repetition set publishes its own immutable attempt seal.
Those observations remain available if a later scenario or the campaign is
interrupted. After scenario results are written, the runner writes suite/JUnit/HTML records,
marks the manifest complete and finally creates `integrity.json`. Interrupted
unwinding instead records an interrupted manifest and attempts its own seal.
Abrupt process termination can leave the active scenario without a seal;
previously published scenario seals remain usable.

History pages and Markdown remain navigation and presentation, never proof
input.
