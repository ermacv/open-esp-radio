# HIL host setup and operations

Run commands in this guide from the repository root. HIL executes the
production driver on real hardware and records typed evidence; it is not an
alternative radio implementation. Read the
[execution and evidence architecture](architecture.md) before interpreting a
bundle.

```text
hil/
├── protocol/          host/target command and telemetry wire protocol
├── schema/            evidence contract
├── model/             the typed scenario model: spec, purpose, verdict, names, derived waits, stand profile
├── trace-header/      the no_std header of conformance traces: typed bounds and clock domain
├── observer/          the observer's build identity and content store
├── scenarios/         versioned, non-secret host workloads and criteria
├── host/
│   ├── cli/           stand CLI: stand commands, jobs, run store, runner launch
│   ├── runner/        runner CLI, the run core and the family registry
│   ├── workload/      repetition context, typed results, boots, fixtures, the family contract
│   ├── lab/           a run's laboratory config and lock, recovery, post-mortem
│   ├── stand-host/    stand discovery, host doctor, fixture probes, the SSH helper
│   ├── flash/         the flash operation: lease, write, journal, start
│   ├── link/          the DUT session, generic exchange, reboots, the peer console
│   ├── net-traffic/   network sessions, paced UDP/TCP and ICMP host traffic
│   ├── run-bundle/    the run bundle: format, writer, the one reader, run store, firmware records, replay checks
│   ├── analysis/      run queries, the one measurement aggregation and comparison, performance, views
│   ├── experiment/    the one run launch, revision runs, A/B comparisons and bisection
│   ├── image/         image class specs for the image pipeline, directories of built images, firmware records, compare_images
│   ├── scenario/      scenario envelope, catalog, campaign plan and requirements
│   ├── executor/      typed scenario execution on the model or the stand, the tier plan and its leases
│   ├── image-class/   image classes and the keys each serves
│   ├── board/         board I/O: ports, the flash writer, starts, resets, OpenOCD, power, consoles
│   ├── family/        one radio family's scenarios and workload each; the Wi-Fi fixtures
│   ├── arbiter/       leases, claims, balances, lock files, job tickets, the board journal
│   ├── fixture/       finite Linux helpers and their contract
│   ├── fixture-install/ root-executed fixture installation
│   └── linux-net/     privileged Linux AP/monitor fixture
├── peers/             ESP-IDF reference peer images (esp32c5)
├── bootloaders/       second-stage bootloader per chip
├── agent/            chip-independent HIL agent logic
└── targets/
    └── esp32s31/      current embedded target workspace
```

Target firmware lives under `hil/targets/<chip>`. Runs are immutable bundles
in the run store (`target/hil/runs`); Markdown is not proof input.

Vendor-linked oracles remain isolated under `verification`; they are
not HIL scenarios or runner commands.

The ownership map and bundle contract are in the
[execution and evidence architecture](architecture.md).

`cargo hil` is the binary of [`oer-tool-hil`](cli/README.md), apart from
xtask. It handles the
stand's commands itself and forwards every other command to the runner: it
builds the observer and runs its private copy. Cargo uses `--locked` and, like every Cargo
command in this repository, runs offline (`.cargo/config.toml`); `cargo tidy
fetch` downloads what a changed lock file needs. Cancellation of
`cargo hil` is forwarded to its owned runner process group, with up to five minutes
for fixture cleanup and finishing the run; this does not limit campaign runtime.
The wrapper returns the runner's exit code (or `128 + signal` on Unix).

Build the observer without running HIL, printing its private copy, with:

```console
cargo hil observer
```

## Guides

| Guide | Content |
| --- | --- |
| [Sharing the stand](stand.md) | Leases, owners and balances, boards and their reset paths, the ESP-IDF firmware catalog, host fixture probing, the dashboard |
| [Runs and investigation](runs.md) | The shared run store, failure post-mortems and board recovery, enqueued runs, bisection, revision runs, A/B comparison, performance, profiles and layout seeds |
| [Host fixtures](fixtures.md) | Network fixture preparation and restoration, evidence boundaries, Linux fixture software installation, probe load and air observers |
| [Execution and evidence architecture](architecture.md) | Package ownership, the run lifecycle and the bundle contract |
| [Stand arbiter](../../stand/arbiter/README.md) | The queue, claims, balances and state files |

## Configure the stand

Run the host interface through the workspace alias:

```console
mkdir -p ~/.config/open-esp-radio
cp host/lab/model/stand.example.toml ~/.config/open-esp-radio/stand.toml
chmod 0600 ~/.config/open-esp-radio/stand.toml
cargo hil doctor
```

The stand file is `~/.config/open-esp-radio/stand.toml` (or under
`$XDG_CONFIG_HOME`), shared by every checkout of this user; `--stand-file`
names another file. Its schema and rules, the one resolver of a board name and the paths of the
stand's state belong to [`oer-lab-model`](../../stand/file/README.md). It is the only source for
the stand's identity, its hubs and pool of boards, STA/AP credentials and
addresses and the fixtures; it lives outside the repository, and scenarios
contain no lab secrets or machine-specific paths. The stand id and the
boards' ids are written into every run manifest so results from different
stands and boards cannot be silently mixed.

A run's device under test is a board of the pool with the `dut` role and the
run's chip, which has a profile in `platform/<chip>/chip.toml`. There is no
default chip: a command that uses a device under test takes it from its
`--chip` or from the only chip that builds its scenarios' images, and checks
and resolves only that board, so an absent board, or one whose chip has no
profile yet, fails only the runs that take it. With several candidates of the
chip, `--board ID` names one, until the stand's scheduler assigns boards.

Peer scenarios run against a reference peer board, an ESP32-C5 with an
ESP-IDF catalog image: the [IEEE 802.15.4 peer](../peers/esp32c5-i/README.md),
the [Thread peer](../peers/esp32c5-openthread/README.md) or the
[Bluetooth LE Direct Test Mode peer](../peers/esp32c5-ble-dtm/README.md).
The peer is a board of the pool with the `peer` role and the chip the run's
peer images target (`hil/peers/*/firmware.toml`), other than the device under
test; boards of other chips never compete for it, and `--peer-board ID` names
it when several of that chip qualify. A run's peer images share one chip.
A run of a peer scenario claims the peer board beside the device under test
and the air in its one lease, and locks both boards. Before the first
scenario that needs an image, the runner brings the board to that image's
current catalog build through the flash operation's catalog flash
(`oer_hil_flash::catalog`, unless the board journal says the board carries
it), and writes the image, application digest and commit the board journal
recorded for it into the scenario's `peer-image.json`. Each
workload takes the running peer over with its `SYNC` command instead of a
reset: a USB Serial/JTAG reset of an ESP32-C5 whose radio runs can leave it in
ROM download. The peer drivers share one console, `oer_hil_link::peer::PeerConsole`:
it owns the whole `@` line grammar (`@READY`, `@OK`, `@ERR` and typed
reports), and records the transcript of every exchange.

## Build and run

`cargo hil plan <scenario>` prints the selection's plan without opening a lab
configuration or device: each scenario's procedure digest, image, repetitions,
requirements and the named checks it supplies. `--proof <check>` filters the selected scenarios by an actually implemented
named check. No current scenario supplies one: the Wi-Fi and Bluetooth
families whose scenarios went in  and . Repeated `--proof`
arguments require all named checks, and `--tag` can restrict the profile:

```console
cargo hil plan --proof <check>
```

The plan lists provided checks and selection reasons; it does not infer success
from tags, select by changed files, or claim minimum coverage of an arbitrary
product program. Selection never expands beyond the requested scenarios.
Scenario digests drop null values, fill schema-5 defaults from
`host/hil/model/scenario-v5-defaults.json` (generated from the typed scenario
families by the runner's catalog test) and exclude top-level `description`,
`role`, `tags` and `unsupported`; all execution fields remain bound, including
repetitions and the complete family table. A plan neither executes hardware
nor invalidates finished observations.

`cargo hil run <scenario>` builds and flashes the required image before the
scenario. `cargo hil image build <class>` builds the class's image bundle through the one image pipeline
([`oer-image-build`](../../tools/image/pipeline/README.md)) with the same stack/move,
placement and packed-image gates, encoding the bootloader, partition table
and OTA selection at build time, without flashing or loading private stand
file. Each successful build emits one JSON report on stdout with class, chip,
target, profile, network, the bundle directory, its runtime ELF, application
image and digest, its gate reports and their warnings; diagnostics stay on
stderr. An ELF or `application.bin` left beside a failed build is not a
successful image report. `cargo fw flash --device MAC|PORT <bundle>` writes a
bundle to an attached board; a run writes its images through the flash operation ([stand](stand.md#the-flash-operation)).

A run takes its images as a directory of built images (`images.json`
beside each class's bundle, with the SHA-256 of every file): `cargo hil run`
builds them into `target/hil/<chip>/images/<run>` from this checkout, or from
the tree `--source DIR` names (an A/B arm's or a revision's worktree); `cargo
hil run --images DIR` flashes images built before, and refuses one whose files
changed. `cargo hil run … --enqueue` builds the run's images when it is
enqueued into the job's own directory (`target/hil/jobs/images/<job>`) and the
job flashes exactly those, whatever the checkout holds when it starts. `cargo
hil sweep` removes these directories once they are unused for the image cache
age. A run records each image's files by their SHA-256 and the commit of the
tree it was built from, with whether that tree was dirty.

Every image build compiles in one shared cache,
`~/.cache/open-esp-radio/build/cargo/`; `cargo hil images check` builds its classes there too. At most half the host's cores, and no
more than its memory holds at 2 GB each, compile image runtimes at once across
all checkouts; further builds wait for a free build slot in
`~/.cache/open-esp-radio/build/tokens/`.

`cargo hil run S... --repetitions N` runs each selected scenario N times
(1 to 20) instead of its own count, for a quicker look while debugging. The run
records the reduced count, which a requirement's minimum repetitions then judge.

`cargo hil run-all` runs the scenarios carrying each `--tag`, or the whole
catalog only with an explicit `--all`. It reuses each image across its scenario group but
does not fail fast. Every invocation retains an immutable evidence bundle in
`target/hil/runs/<run-id>/`, including a canonical JSON suite, JUnit
XML, a standalone HTML report and the exact application image flashed for each
firmware class. The flash operation reads that archived copy, binding firmware
provenance to the bytes sent to the DUT. A bundle carries no seal: a run of
another `RUN_SCHEMA` is not read, and stays in the store until it is deleted
explicitly (`cargo hil runs prune --unreadable --apply`).

The manifest records `runner.observer`, including the SHA-256 of the running
executable and its embedded build/source identity. Firmware capture does not
replace this identity.
[`observer-inputs.json`](../model/observer-inputs.json) selects the relevant
observer inputs; equality of the entire runner binary is not required.
Its schema-4 registry identifies a workload as `<family>/<kind>` (for example
`system/boot-smoke`), classifies its timing sensitivity, and scopes its inputs by
the `common` group plus the scenario family's group (`system`),
whose direct dependencies are the runner family
packages. A workload's source inputs are the runner package, every path package
in that projected dependency closure and the listed non-Cargo `data` files;
source paths are never enumerated.
Legacy bundles need an explicit provenance review before becoming applicable.

## Inspect evidence

`cargo hil runs history <scenario>` reads a scenario's outcomes and
measurements straight from the bundles in the store and starts with its pass
rate and how many of its newest runs in a row did not pass; there is no
derived history file to rebuild. Trends are scenario aggregates, not a proof
of comparable firmware or fixture conditions.
