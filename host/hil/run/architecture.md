# HIL execution and evidence architecture

This document defines the host runner's ownership and evidence bundle
contract. Operational setup and commands are in the
[HIL host guide](README.md).

The runner owns the typed CLI, scenario catalog and build/flash
orchestration; the family crates own their workloads, and the link owns the
UART evidence. `linux-net/` contains only privileged fixture operations.

Separate Cargo packages bound the privilege and radio-family scopes:

| Package | Binaries | Role |
| --- | --- | --- |
| `cli/` (`oer-tool-hil`) | `oer-tool-hil` (`cargo hil`) | The stand's commands, as argument parsing and calls into the stand packages below (leases, boards, owners, devices, stand discovery and doctor, fixtures, flash, the ESP-IDF firmware catalog), the analyses (`runs`, `perf`, the dashboard's runs), the experiments (`ab`, `bisect`, `revision`) and the launch of every run through `oer-tool-hil`; the frozen binaries and detached processes of jobs |
| `runner/` (`oer-tool-hil-runner`) | `oer-tool-hil-runner` | Unprivileged CLI and the run core: selection, lease, images, repetitions and finishing the run. It names the family crates once, in its family registry (`scenario.rs`), and reaches every family, fixture provider and preflight through it. Its one extension point is `run --then` ([stand guide](stand.md)), a shell command inside the run's lease after the scenarios that is recorded as events and never changes the outcome |
| `workload/` (`oer-hil-scenario`) | none | What a workload runs against: the repetition `Context`, its one result writer (`results`, written as `observations.json`), `for_each_boot`, `require_keys`, failure classification, per-repetition cleanup evidence, the measurements recorder, the type-erased `Fixtures`, `BoardImages` (the run's writer of the board under test's images, through the flash operation), and the family registry contract (`family::{Workload, Kind, Registry, FixtureProvider}`) |
| `lab/` (`oer-hil-lab`) | none | A run's stand operation: its laboratory configuration, the fixture lock it holds, the cell's pre-run observation, fixture software leases, recovery and post-mortem; implements the link's `Dut` port. It actuates no hardware and writes no flash record |
| `link/` (`oer-hil-link`) | none | The host/target link: one DUT session (`SerialCapture`) with the generic exchange (`call`, `request`, `command`, `wait_command`) and reboots; reaches the board only through its `Dut` port; and the reference peers' `PeerConsole` with the whole `@` line grammar (`peer`) |
| `image/` (`oer-hil-run`) | none | The image builder: firmware construction from a tree (the checkout or an arm's worktree) into a directory of built images, placement and stack audits, and the firmware records it hands to evidence |
| `run-bundle/` (`oer-hil-runs`) | none | The run bundle: its typed documents, the one writer, the one typed reader (`RunBundle`) every consumer uses, the run store with its sidecars (`RunStore`), the launch run list, the content-addressed object store, the replay checks of archived firmware against its digests, and the experiment and laboratory records a run keeps |
| `analysis/` (`oer-hil-analysis`) | none | Analyses of run bundles: the one measurement aggregation and comparison, run queries, retention, performance baselines, A/B arm comparison, the dashboard's runs and the JUnit/HTML views a run writes |
| `experiment/` (`oer-tool-hil`) | none | `launch_run`, the one way a run is launched (its runs come from the runner's launch run list), the job a launch runs as, revision runs (a revision's scenario set with the runner of its HIL wire), A/B experiments and bisection |
| `scenario/` (`oer-hil-scenario`) | none | The family-independent scenario envelope, catalog and campaign plan, with the laboratory requirements, Wi-Fi link vocabulary and target settings a scenario declares |
| `image-class/` (`oer-hil-model`) | none | Image classes, their build features and the image keys each class serves, the check of a device's reported keys against them, and where a chip's HIL images are built from (`agent`: the agent's workspace, package and manifest, the bootstrap) |
| `family/system/` (`oer-hil-family-system`) | none | One scenario family: its scenario table (`ScenarioFamily`: validation, plan, requirements, image keys, peer image, air use), its `Workload` and its target exchanges; each depends on `oer-hil-scenario`, never on another family |
| `family/phy/` (`oer-vendor-phy`) | none | The host half of the PHY vendor calibration cross-check: the comparison port and the vendor IEEE 802.15.4 reference firmware driver; no scenario family registers it; it names no chip |
| `family/phy-esp32s31/` (`oer-vendor-phy-esp32s31`) | none | The ESP32-S31 comparison port: `oer-vendor-phy-esp32s31` behind it |
| `family/ieee80211-fixture/` (`oer-hil-family-ieee80211-fixture`) | none | The Wi-Fi fixtures (local Linux and OpenWrt access points, hostapd, host network routes, air monitors), their `FixtureProvider`, and the air-capture and BSS-protection analyses |
| `net-traffic/` (`oer-hil-net-traffic`) | none | Host traffic against the target's network sessions: session start and evidence, readiness, paced UDP and TCP, offered load and the one ICMP method (datagram sockets) |
| `../../stand/file/` (`oer-lab-model`) | none | The stand file, the one resolver of a board name (id, chip or MAC) and the XDG paths of the stand's state |
| `../../stand/board/` (`oer-lab-board`) | none | A stand board: its leased device (the device lock of `oer-lab-board`), the receipted image write of `oer_lab_devices::image`, starts, the reset ladder, hub power and consoles |
| `../../stand/arbiter/` (`oer-lab-arbiter`) | none | Claims, the queue and its job tickets, balances, preemption, maintenance; a board whose device lock a foreign process holds is busy |
| `flash/` (`oer-hil-run`) | none | The flash operation of HIL: lease, write, journal, start; and the ESP-IDF catalog flash |
| `../../stand/discover/`, `../../stand/doctor/`, `../../stand/ssh/` | none | The stand's host: discovery of its boards, the host doctor, fixture reachability and the one SSH helper to its OpenWrt hosts |
| `fixture/` (`oer-hil-fixture`) | `open-radio-probe` | Finite Linux helper; the library is its versioned request/report contract with the runner |
| `../../stand/fixture-install/` (`oer-lab-fixtures`) | `open-radio-fixture-install` and the two fixed launchers | Root-executed installation and admission; the runner uses the same library to plan and prepare |

The installer package depends on no radio or HIL execution crate, so
its dependency graph is the whole root-executed installation surface.

The three packages deny `unsafe_code` at their crate roots, and the helper and
installer binaries forbid it. System calls go through `rustix`. Raw memory
reaches the kernel in three places, each with its `SAFETY` justification:
`host/os/process/src/linux_socket.rs` for link-layer, HCI and L2CAP addresses,
`BT_SECURITY`, `SO_MEMINFO` and `SO_BINDTODEVICE`; `fixture-install/src/launcher/handoff.rs` for the
lease descriptor passed across `exec`; and the runner's adoption of the serial
port descriptor.

Fixture installation has a separate ownership boundary. The general runner
owns the offline provider plan, unprivileged locked build, content-addressed
bundle and foreground sudo handoff. The narrow `open-radio-fixture-install`
binary owns only fixed Linux destinations, root-owned import, candidate/effective
policy validation, generation activation, recovery and the installation
receipt. It cannot select an arbitrary root, output path or command. A single
persisted journal covers policy and stable links; the atomic `current` symlink
switch is the commit point. Previous generations remain available for rollback.
One root-owned lock serializes all applies. Shared provider software leases span
HIL fixture/run ownership, while apply requires the selected provider's
exclusive lease. Each persistent provider lease is a stable inode under that
provider's installation state. Operational admission checks the journal,
selector, receipt and selected artifact under the shared lease. Fixed finite
launchers perform this admission and bind a concrete generation before direct
network or probe helper execution; they neither recover nor install.
These locks never acquire a device or replace the physical fixture leases below.
The stand's arbiter orders both sides: a run claims
`fixture-software:<provider>` shared and takes the software lease only once
granted, and `cargo stand fixture install` claims it exclusively before `sudo`.
An installation therefore waits only for runs using its provider, never for a
queued run, and runs queued after it wait for the new generation.

The runner entry point in `runner/src/main.rs` registers the executable's build
identity with `oer-hil-runs`, then maps the top-level result to the process exit
status. `runner/src/command.rs` owns CLI
startup and command-specific dispatch. Run selection and the suite/scenario/
repetition lifecycle are in `runner/src/execution/orchestration.rs`, while
`runner/src/execution/firmware.rs` coordinates run-local build or replay
publication before calling the existing image and device owners.
`runner/src/execution/preflight.rs` owns run selection compatibility and
hardware-facing scenario/image checks, and `execution/doctor.rs` the
selection-scoped environment report; declarative resource discovery remains
under `oer_hil_scenario::requirements`. Machine JSON retains its dedicated descriptor in
`workload/src/output.rs`. Workload dispatch through the family registry and
typed execution evidence remain in `runner/src/execution.rs`; a workload's
typed results leave its repetition through `Context::finish`, the one
`observations.json` writer; `oer_hil_run_bundle::run::RunSession` is the sole run
writer.

## One run lifecycle

The implementation order matters because later files must not retroactively
claim an earlier physical action:

| Order | Owner and durable result |
| --- | --- |
| 1. Selection and plan | CLI/catalog code resolves typed scenarios. `RunSession` creates a unique directory and writes `plan.json`. |
| 2. Firmware archive | `image` builds every selected class whose scenarios meet their configuration preconditions, before the stand is leased. `RunSession::record_firmware` stores the subjects and returns the run-local `firmware/<class>/application.bin`. An explicit replay source is checked by `oer_hil_run_bundle::replay` under the lease. |
| 3. Leases and lab provenance | The runner waits for the [arbiter](../../stand/arbiter/README.md) lease on the boards, fixtures and air the selection claims. The run's fixture lock then takes the arbiter's lock files of its boards (by MAC), the local wiphy and the managed OpenWrt host its selection uses; these are local user-account locks, not distributed reservations. While they are held, `lab::provenance` records the secret-free topology before any flash. |
| 4. Flash | The flash operation (`oer-hil-run`) writes the bundle around that archived application under the run's lock of the board, through board I/O's one writer, journals it and starts it as the chip profile says. The workload never flashes a different build-tree copy. |
| 5. Repetitions | `fixture::prepared` owns peer/host preparation; `session` owns serial reset, raw `uart.bin`, decoded protocol and target-health state; a workload owns its child processes and typed observations. Primary failures remain distinct from infrastructure failures. |
| 6. Cleanup and attachment indexing | Each repetition enters a cleanup scope before fixture preparation. Cleanup/restoration finishes before attachments and `result.json` are collected. `cleanup.json` preserves every attempted restoration and its failure independently of the workload result. |
| 7. Suite | After all scenario results, `RunSession::finish` writes `suite.json`, the JUnit and HTML views `oer-hil-analysis` renders, and the final event/manifest. |

SIGINT/SIGTERM wakes owned protocol and process waits. Ordinary unwinding
finalizes captures, restores fixtures where possible, marks the run
`interrupted` and keeps the partial files.
Cleanup failure is reported and can quarantine subsequent network workloads;
it never replaces the workload cause or creates a pass. Abrupt termination
cannot run those owners' destructors and may leave only incrementally written
UART bytes and a manifest still recorded as running.

The flash operation owns the write, a capture owns its serial process and
files, and each fixture owner may restore only the interface/process it
created. Loss of SSH or an identity mismatch can make restoration impossible;
the runner records that ambiguity instead of claiming a reusable lab state.

The runner's own commands follow; sharing the stand, investigating runs and
fixture installation are described in [stand](stand.md), [runs](runs.md) and
[fixtures](fixtures.md):

```console
cargo hil doctor
cargo stand fixture install --provider linux-net --dry-run
cargo stand fixture install --provider linux-net
cargo hil doctor boot-smoke
cargo hil plan boot-smoke
cargo hil scenario list
cargo hil scenario validate [id]
cargo hil image build <image-class>
cargo hil run <scenario-id>
cargo hil run <scenario-id> --firmware-from <run-id>
cargo hil run-all --role qualification  # or --all for the whole catalog
```

`plan [scenario] [--tag ...]` resolves requirements from the catalog offline;
it does not read the stand file, inspect tools or contact hardware. `doctor`
accepts the same selection and reports all independent environment checks as
JSON, returning nonzero if any fail. With no selection it checks the whole
catalog. It checks build/flash tools, scenario preconditions, required fixture
services and current cooperative resource availability; it neither flashes nor
resets the target. Availability is an observation, not a reservation for a
later run. Optional monitor tools are checked only when selected evidence uses
them.

Scenarios are versioned TOML files in domain folders under `hil/scenarios`; they contain one
family's workload and acceptance criteria, never serial paths or secrets.
`oer-hil-scenario` owns the family-independent envelope, catalog and `Plan`
projection (image, laboratory requirements, target initialization, named
checks and Wi-Fi laboratory use); each family package owns its typed table,
validation and execution, and the runner composes the families. Machine-local
boards, STA/AP and OpenWrt values live only in the mode-0600 stand file
(`~/.config/open-esp-radio/stand.toml`).
`LabConfig` is immutable. Each workload receives its own execution context:
borrowed laboratory inputs and the selected scenario's initialization settings.
Experiment policies are never written back to the shared laboratory object.
The CLI grammar lives in `runner/src/cli.rs`. Scenario dispatch passes typed
workload configurations directly; workloads neither rebuild CLI arguments nor
parse private command-line dialects. Serial ownership comes from the execution
context, independently of traffic configuration. Workload limits and acceptance
policy remain enforced when constructing a running workload.

By default, `run` builds and flashes the scenario's exact image before
executing it. An explicit `--firmware-from` selects exact artifact replay
instead, never a rebuild. `run-all` groups scenarios by image class, so changing
a workload's parameters does not rebuild or reflash firmware. It continues
after scenario, image-build and image-flash failures, records the remaining
scenarios as blocked when necessary, writes the complete suite, and returns a
non-zero status unless every selected scenario passed. Every scenario resets
the target.

Each invocation creates an immutable directory under
`target/hil/runs/<run-id>/`. The runner never deletes or reuses an old
run. Its canonical records are:

```text
manifest.json       invocation, repository, host, lab and firmware provenance;
                    when the run finishes, `messages_used`: every message path sent or
                    received, sorted and unique
lab-provenance.json secret-free pre-run topology, host and fixture observation
plan.json           selected and filtered catalog entries
events.jsonl        append-only execution timeline
suite.json          typed suite/scenario/repetition outcomes
junit.xml           CI view derived from suite.json
report.html         human view derived from suite.json
firmware/<image>/
├── application.bin exact application bytes used by the flash operation
├── runtime.elf.deflate
│                   exact symbolized runtime used to produce the image,
│                   deflate-compressed; recorded as runtime.elf
├── runtime.bin     exact packed stage-two runtime
├── bootstrap.elf   exact bootstrap used to encode the application
│                   (bootloader.bin and partition-table.bin instead of
│                   runtime.bin and bootstrap.elf for an ESP-IDF application)
└── effective-Cargo.lock
                    embedded dependency resolution observed before restore
scenarios/<id>/
├── scenario.json
├── result.json
└── repetition-NNN/
    ├── result.json
    ├── host-route.json
    ├── cleanup.json
    ├── measurements.json
    └── workload evidence
```

`lab-provenance.json` is collected while the fixture lock is held, after the
current-source images are built and before any flash. Its explicit `system` scope collects OS facts and sysfs
interface identity without calling `ip`, `iw` or SSH; network fields are not
observed and the fixture is `not-used`. Network workloads use `network` scope.
A replay of the run's firmware requires a matching archived plan and scenario
snapshots before accepting system-only provenance. It deliberately omits both network credentials and
transport endpoints. For a managed OpenWrt fixture it records the actual
release/kernel/boot identity, driver and firmware, country, TX power, channel,
frequency, width, associated-station count and concurrent VIFs. Host interface
and route-table state are recorded at the same boundary. The target-specific
route cannot exist reliably at that point, so every station traffic repetition
later writes `host-route.json` after address assignment and fails unless the
socket source and required Ethernet/WLAN medium match the kernel route.

Repetition records index every evidence attachment with its relative path,
media type, byte length and SHA-256 digest. Schema 2 repetition records also
carry typed measurements: a stable name, integer value, unit and, where the
scenario has a gate, its comparator, threshold and independently computed
verdict. Every workload family receives one repetition-owned recorder through
its execution context. Captures project decoded protocol values into numeric
observations and save a `measurements.json` beside `protocol.jsonl`, including
on error or unwinding. Repetition results include the same observations.
Names distinguish boot/capture paths, sessions, requests and individual flows;
`network::ReplayResult` cannot duplicate or replace the first traffic observation.
Transport rates use the target's reported elapsed time; zero elapsed time
produces no rate. Link counters are explicitly named as lifetime observations.
The projection includes transport, link/stack, timer, scan, monitor, AP peer
and ED polling measurements; other typed facts remain in `protocol.jsonl`.

Workload-owned measurements also expose their existing acceptance floors.
These are the resolved predicates, not new criteria. Target observations have
no implicit verdict, and numerical observations alone do not qualify a radio
feature. Broken or interrupted repetitions can retain failed measurements;
a passed repetition cannot. Rendering never parses Markdown to decide an
outcome. Host ICMP and TCP measurements are recorded before UART finalization,
so a later link failure does not discard completed host observations.
The HTML run report groups measurements by repetition in expandable sections.

The context owns the shared capture lifecycle: cancellation check, output-scope
validation, reset, observation collection and finalization. Bounded control
and probe operations use `with_capture`; concurrent traffic owners can keep an
explicit capture handle. Both paths retain primary and teardown errors, and
ordinary unwinding saves partial observations. Concrete fixtures still own
restoration of their AP/client/monitor state.

SIGINT and SIGTERM cancel protocol waits, paced traffic and supervised host
commands. The active repetition is saved as `interrupted`; execution does not
start another repetition or fabricate results for unexecuted scenarios. The
manifest retains the partial run, and stdout reports its
location. An interrupted run does not require a completed suite.

Fixture owners restore partially configured resources on errors and cancellation.
`cleanup.json` records restoration attempts, elapsed time and failures separately
from the primary workload failure. Restoration runs within a bounded cleanup
scope (30 seconds, shared by nested operations). A cleanup failure alone makes
the repetition `broken`; it cannot turn a failed workload into a pass.
OpenWrt client preparation installs host recovery before restarting wireless;
the remote restart also restores wireless on ordinary shell exit or signals.
TX-monitor ownership starts before spawning SSH or waiting for readiness. A
private remote directory identifies that capture's resources, so a rejected
pre-existing monitor is left intact. Remote traps remove the owned interface;
host cleanup retries removal and deletes the capture directory, with failures
recorded in `cleanup.json`. Loss of SSH connectivity can prevent restoration;
the runner reports that failure rather than claiming the fixture was restored.
OpenWrt client cleanup checks the remaining NAT/forwarding rules and managed
interface. An already absent resource is safe to retry; an inspection or
deletion failure is recorded. Before signalling a stored PID, cleanup checks
that its command is `wpa_supplicant` with the owned interface and configuration;
a different command is left intact and reported as a recovery failure.
AP management, packet capture and fixture snapshot failures carry a typed
fixture error and produce `broken/infrastructure`, including errors returned
by the secondary-client probe thread. Actual peer packet loss remains a
scenario failure. A radio configuration mismatch
reports the required channel/width and the observed channel line without
including network credentials. These failures do not change scenario criteria.

The laptop helper contract is schema 6. Its `client` action returns status 10
only when a prepared client exhausts the association wait; command failures
and malformed supplicant status are infrastructure errors. `doctor` and the
selected run preflight reject older helpers before flashing or resetting the DUT.
Provision it with `cargo stand fixture install --provider linux-net`
from the repository root before using laptop client scenarios. Installation
performs software-only verification; it never substitutes for this helper
preflight or a fixture/hardware check.

`oer-os-process` owns local child process groups, drains captured stdout/stderr
concurrently and stops descendants on cancellation, deadlines or owner drop.
Routine commands have a 120-second deadline; image commands allow 30 minutes;
packet captures use their configured duration plus shutdown allowance. Remote
process lifetimes additionally depend on the OpenWrt scripts' timeouts and traps.

Serial-device and fixture leases live in the user's host cache, outside
individual checkouts. A board itself is held through the device lock (`oer-lab-board`, keyed by
its MAC), which `cargo fw`, `cargo stand`, the runner and the verification
captures share.
A run additionally leases every required local wiphy and the managed OpenWrt
host boot. Local client/monitor interfaces sharing a radio conflict. The remote boot identity makes different SSH aliases and radio
interfaces on one OpenWrt host conflict; the whole host is reserved because
client setup also changes firewall state. A remote reboot invalidates that
fixture epoch. These are cooperative locks between runners on this host and
user account, not distributed reservations across separate laboratory hosts.
External unmanaged APs have no discovered physical identity and are not
claimed. Build/flash-only commands and device inspection acquire no
AP or laptop-radio resources.

The [stand arbiter](../../stand/arbiter/README.md) orders these locks. Every command that
takes them first waits for a host-wide lease on the resources it claims; the
locks remain the final exclusion, and a granted holder waits for any lock
still held by a process outside the queue. A run holds the lease from its
first flash to its end, not while it builds; a run of several scenarios may
yield it between scenarios and flashes its image again after the next grant.

A session unwound by a runner error is marked interrupted in the manifest.
Each UART capture owns its output directory before opening or resetting the
serial device. `uart.bin` is the exact received stream, written as bytes arrive;
`uart.log` is its lossy UTF-8 view for diagnostics. `protocol.jsonl` contains
a `host-command` record for every command the host sent (request id, session,
variant name and host send time, never the payload), received target events
with their host receive time, decoder counters, a `capture-end` record with
the first typed link failure, and the final target-health query when
available. Host
commands and host error messages are never inserted into the received stream.

Serial open, reset, read, write and worker failures wake protocol waits. Decode
errors, receive overflow, sequence gaps and an unexpected new boot invalidate
the capture. A later boot cannot erase an earlier failure: each intentional
reset starts a new capture. Optional event waits distinguish a healthy timeout
from a broken link. A traffic result has one collection deadline; target
session and Wi-Fi operation failures terminate their corresponding waits.
Before acknowledging a completed traffic session, the host requests its
retained result again and requires identical evidence and completion digest.
These protocol exchanges run after the measured traffic interval.

Host I/O and typed link failures produce `broken/infrastructure`; scenario
assertions produce `failed/scenario`. Operation context preserves the typed
cause. Capture finalization saves partial evidence before returning a link
failure; when both the scenario and finalization fail, the report retains both
messages and classifies the primary cause. Ordinary early returns also save
the decoded transcript through the capture's destructor. Abrupt process
termination can leave only the incrementally written raw bytes; it does not
run Rust destructors or finish a completed run.

Reconnect stores captures per boot. The command emits one completion
JSON object on stdout; diagnostics, progress and inherited child-process output
belong on stderr.

A run's directory is created with a unique name and needs no lock shared
with other runners. The runner keeps no derived view over other runs:
`cargo hil runs list`, `runs history` and the dashboard read the bundles
themselves.

A run carries no seal. A bundle is the files its writer left; a run of
another `RUN_SCHEMA` is not read (`schema N, expected M`) and stays in the
store, which deletes it only on `cargo hil runs prune --unreadable --apply`.

Runs retain all firmware subjects through a SHA-256 content-addressed
store under `target/hil/<target>/objects/`. The files inside a run are ordinary
hard links when the filesystem supports them, or independent copies
otherwise. This keeps a copied run self-contained without allocating another
large runtime ELF for every repeated scenario. Each image's record in the
manifest is its files by their SHA-256 and the commit of the tree it was
built from, with whether that tree was dirty.

`cargo hil run <scenario-id> --firmware-from <run-id>` checks the source
run's manifest, laboratory provenance and firmware records offline
before acquiring the physical fixture, requires the archived
image class to match the selected scenario, and then executes the ordinary
scenario lifecycle without invoking Cargo. The resulting run imports all
available firmware subjects into its own CAS-backed bundle; its firmware replays again after the source run
is removed. Its manifest records the source run.
`run-all` does not accept `--firmware-from`.

The derived views and Markdown narratives are never proof inputs.

`boot-smoke` intentionally precedes the radio protocol and proves only runtime
relocation plus one Embassy timer wake. It uses its single fixed PASS record;
all radio, lifecycle and traffic evidence uses the typed HIL protocol.

## Source ownership

The host packages own these roles:

- `oer-hil-scenario` owns catalog values, discovery, semantic acceptance
  rules and the campaign plan, with the requirements, Wi-Fi link vocabulary
  and target settings a scenario declares; `oer-hil-model` owns image
  identities, their feature recipes and the keys each class serves.
- `oer-hil-runs` owns the bundle's typed documents, the run writer,
  the one typed reader, the run store and its sidecars, the launch run list,
  the content-addressed object store, and the replay
  checks. It depends on no stand, board or image
  builder code: the image builder hands it firmware records,
  the runner hands `RunSession::finish` the renderer of its HTML/JUnit views
  (`oer-hil-analysis`), and a run reports its interruption through a
  callback its caller publishes. `oer-hil-analysis` reads bundles for every
  query, statistic and view; `oer-tool-hil` launches every run, and `oer-os-durable` provides atomic files,
  digests and timestamps to every producer.
- `oer-hil-run` turns an image class into an image spec for the one image
  pipeline, [`oer-image-build`](../../tools/image/pipeline/README.md) (the class's features
  and network, each chip's agent and HIL stack policy, the radio observers'
  placement audit), builds a run's images into a directory of built images
  (`images`) and writes the records it hands to evidence (`record`). The pipeline compiles, runs every gate
  (over `oer-analysis-riscv-stack`) and encodes the bundle's flash files, the
  ESP-IDF catalog bootloader of an ESP-IDF chip included. The builder does
  not print: `artifact_report` returns the report the CLI publishes.
- `oer-hil-lab` owns local configuration, the pre-run observation of the
  cell, the exclusive fixture guard, fixture software leases and the
  laboratory error type, and implements the link's ports for the leased
  board. Each family
  package's `fixture` implements its controlled host and peer capabilities:
  the Wi-Fi `local` (laptop radio and helper) and `openwrt` (SSH-managed
  router) providers each own their AP, client, monitors and session evidence,
  `controlled_ap` selects between them and `prepared` owns a scenario's AP
  lifetime. The runner binary runs every family's fixture preconditions
  before a scenario.
- `oer-hil-link` owns one UART capture and its protocol/readiness/validation
  state, the host traffic transports and `measurements`, which projects
  decoded messages into measurements. It reaches the board as a `Dut`
  (console and application reset), which the stand implements.
- `context` gives one workload repetition its laboratory, target settings,
  capture lifecycle and measurements; workloads that control the AP receive
  the prepared fixture explicitly.
- Each family's `workload` module owns its operations: the system family's.
  They report scenario outcomes, not product readiness.
- The stand's `post_mortem` asks a failed repetition's target, attached
  without a reset, for its boot evidence and trace, and classifies hangs and
  unexpected resets; its `recovery` owns the reset ladder for a target that
  does not answer and decides whether its board is recoverable or
  quarantined. `oer-hil-scenario`'s `failure` classifies errors as scenario or
  infrastructure failures.

Dependencies form a directed acyclic graph that Cargo enforces: the binary
depends on the family packages and the image builder, each family on
`oer-hil-scenario`, and `oer-hil-scenario` on the stand, link, scenario and
run bundle packages, never the reverse. The stand depends on the link to
implement its ports, never the reverse. The image builder depends on the run
bundle, never the reverse, and nothing but the binary, `oer-tool-hil`,
`oer-tool-hil` and `xtask` depends on the builder. The runner never depends
on `oer-tool-hil` or `oer-tool-hil`, and no library runs `cargo hil`. The `test-support` features of `oer-hil-scenario`, `oer-hil-lab`,
`oer-hil-scenario` and `oer-hil-runs` expose
their test doubles and fixtures to the other packages' tests.

The recursive [catalog contract](../scenarios/README.md) has one discovery,
`oer_hil_scenario::catalog::documents`: the runner parses each document with
its family's types. Tests are adjacent files within each owner.

A capture that begins at a reset expects the boot's unsolicited Hello at
target message zero. The USB Serial/JTAG can drop the first bytes of that
frame, so when no Hello decodes in time, `request_image_keys` asks the boot
for its image keys instead: only when the console already shows the chip
starting (the ROM banner or the bootloader's lines) and no boot was seen yet,
and the answer begins the boot only while it is among the boot's first eight
messages. The capture's link health records it as `solicited_hello`; any other
missing Hello still fails as before.

The watchdog and panic-reset workloads use
`session::reboot` to arm one expected new boot within an explicit interval. It sends no reset command. A fresh Hello
at sequence zero is required; prior transport/protocol failures, an early/late
boot or an additional reboot still fail the capture. All ordinary captures keep
their unconditional unexpected-reboot rejection. Each workload separately checks
the reset cause it expects, so a changed boot ID alone is not evidence of the
mechanism.
