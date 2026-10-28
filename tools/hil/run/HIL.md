# Hardware-in-the-loop infrastructure

HIL runs compiled production code on an attached device and records typed,
immutable evidence bundles. It does not implement radio behavior, and a passed
scenario or internally consistent bundle is not by itself a qualification
decision.

`cargo hil --help` and `cargo stand --help` describe their clap command trees.
Use `cargo hil COMMAND --help` or `cargo stand COMMAND --help` for details.
`--root PATH` and `--owner NAME` work before or after a command name. Queueing
options follow `run`, `run-all` or `ab`, for example
`cargo hil run boot-smoke --owner phy --enqueue --after JOB`.
For a leased program, separate its arguments with `--`:
`cargo stand lease --board dut --owner phy -- sh -c 'cargo hil run boot-smoke'`.
`cargo stand wait --service [BOARD...]` waits for service; `--service` is required.

## Choose a route

| Task | Start here | Hardware or host effects |
| --- | --- | --- |
| Understand execution, cleanup and the run bundle | [Execution and evidence architecture](architecture.md) | Reading only |
| Inspect scenario definitions | [Scenario catalog](../scenarios/README.md) | Reading only |
| Configure the lab and run commands | [Host setup and operations](HOST.md) | The guide labels build, fixture and DUT effects |
| Share the stand: leases, boards, firmware catalog | [Sharing the stand](stand.md) | Leases, flashes and resets of named boards |
| Find, compare and investigate runs | [Runs and investigation](runs.md) | Reading only, except enqueued runs, bisection, revision runs and A/B comparisons, which run scenarios |
| Preview or install Linux fixture software | [Canonical fixture installation](fixtures.md#linux-fixture-software-installation) | Dry-run is offline; installation uses interactive privilege but no RF |
| Build or understand target firmware | [ESP32-S31 target](../../../firmware/hil/esp32s31/README.md) | Build-only sections are separate from flash/run sections |
| Install or operate the Linux network fixture | [`linux-net`](../linux-net/) and its checked helper interface | Requires explicit host setup and, for installation, privileges |

The root Cargo workspace contains both host and target Rust packages. The
wire contract and chip-independent agent are in `protocols/hil/`; images
are in `firmware/hil/`. The executable catalog is in `tools/hil/scenarios/`,
its envelope and workload APIs in `tools/hil/scenario/`, and runner
orchestration in `tools/hil/run/`. `tools/hil/model/` owns typed scenario
values, evidence schemas and image classes; it also has the separate typed
executor in `tools/hil/executor/`.

[`observer/`](../observer/README.md) builds the runner. Its identity format
and content store, together with the bundle format, writer and reader, live
in [`runs/`](../runs/README.md). The PHY family is composed by
`tools/vendor/phy-esp32s31/` over `tools/vendor/phy/` and the chip-neutral
[capture types](../link/src/capture). Its reviewed relation is in
[chip knowledge](../../../chips/esp32s31/knowledge/README.md).
Generated runs are stored outside the checkout, with a `target/hil/runs`
link; they are not tracked. A run never writes tracked files.

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

`cargo hil images check --all` builds the selected chips' available image classes and
checks the linked images. Diagnostic classes use `Required::Diagnostic`:
interrupt stacks may be `partial + ?` with their unresolved sites named, and
strict handler/executor code or executor task code outside its permitted
memory, and breaches of the hardware class, are reported as warnings with
their counts. The paths are in the
bundle's `interrupt-placement.txt`. Other classes use `Required::Product`
and fail these placement violations or an unproven interrupt bound. Both
policies enforce the stack budget, handler entries in SRAM, panic code and
panic-hook data outside cached memory, FPU safety and admitted assumptions.

## Hardware route

Copy `tools/lab/model/stand.example.toml` to the mode-0600 stand file
`~/.config/open-esp-radio/stand.toml`, shared by every checkout, describe
the stand's hubs and boards and the required fixture values, install only the
helpers required by the selected scenario, then run `cargo hil doctor` and
`cargo hil plan <scenario-id>`. Flash, replay, fixture-check and run commands
can reset or modify the named DUT/fixture and require their exclusive leases;
follow the [host guide](HOST.md) for their exact prerequisites and
effects. “Without a DUT” does not mean “without host or peer-adapter changes.”

## Run and evidence lifecycle

The runner resolves the catalog plan and builds the current-source images into
the new run directory. It then waits for the stand's lease from the
[arbiter](../../lab/arbiter/README.md) on the boards, fixtures and air it claims,
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
