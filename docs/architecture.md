# Architecture

What the system is, how it is divided and the rules new code follows.
Details live in the README next to the code they govern; the topic pages at
the end go deeper.

## Goals and scope

- **Chips.** ESP32-S31 is the product; ESP32-C5 is a peer on the vendor SDK.
  Its code lies in the tree outside the Cargo workspace and CI.
- **Radios.** Wi-Fi in every mode, Bluetooth LE (every controller capability
  the hardware offers, multiple connections and the central role first),
  IEEE 802.15.4, with hardware coexistence between them. Bluetooth Classic
  later: ACL first, one Bluetooth driver with LE and BR/EDR phases, a generic
  HCI layer with LE as one sub-dispatcher, one air reservation for all of
  Bluetooth.
- **Vendor parity is not a goal.** Clean architecture with hard boundaries,
  sans-IO protocols tested on the host and hardware offloads come first.
  Vendor code is evidence of hardware behaviour, not a design to copy; a
  vendor algorithm may sit behind a seam as one policy but never shapes an
  interface. What a standard defines follows the standard; a vendor value
  replaces it only with a stated reason.
- **Licensing.** MIT OR Apache-2.0. ESP-IDF ports are Apache-2.0 only, with
  their modification notices. Code written from the vendor libraries and ROMs
  records facts, not a port. Pinned vendor libraries and ROMs may be analysed,
  emulated and linked into comparison images locally and in CI; they are
  never published. Rules for each source: [source policy](source-policy.md).

## Layers

```text
application          credentials, network stack (embassy-net), sockets
  protocol runtime   one async loop per role set, over the protocol's port
  protocols          sans-IO state machines: frames, MAC, RSN, station, AP,
                     BLE link layer and HCI, IEEE 802.15.4 MAC
  radio port         lifecycle and capacity vocabulary shared by the ports
  chip drivers       ESP32-S31 Wi-Fi, BLE and IEEE 802.15.4 drivers
  radio system       one owner of the shared RF front-end
  PHY                calibration, tracking, channel, TX power (Rust port of libphy)
  HAL                register logic over the PAC: root, sequences, polls,
                     routing, clocks, power, resets, DMA engines
  PAC                the exact hardware model, generated: blocks, field
                     accessors, value domains, DMA pointers, ownership parts
platform (firmware/) staged boot, interrupt table, panic record, linker layout
```

Wi-Fi is split into `wire` (over-the-air formats), `port` (the backend
contract), sans-IO cores and a `runtime`; Bluetooth LE and IEEE 802.15.4 have
their port, model and protocol crates and grow the same split. A backend
depends on its protocol's port, its wire formats and `cipher` only.

Protocol cores are **sans-IO**: frames, completed operations and the current
time enter as values; actions and the next deadline leave as values. They
never await, so they run under any executor and in host tests with virtual
time. Only a protocol's runtime awaits, and it binds no executor: time comes
from `oer-time`, interrupt signals from `portable/`, no executor,
`embassy-time`, `embassy-net` or HAL dependency (`embassy-sync` and
`embassy-futures` are allowed). The firmware composition declares the tasks
that poll the futures.
Drivers own hardware and wait; adapters bind executors.

Code whose value is recovered knowledge or a pure algorithm is kept when it
moves; ownership, orchestration and wiring are rebuilt.

## Execution

| Context | Runs | Rule |
| --- | --- | --- |
| P3, core 0 | radio interrupt handlers; one shared radio doorbell on `FROM_CPU` line 3; the IEEE 802.15.4 engine (192 µs turnaround) | Wi-Fi and BLE handlers only latch status and post events |
| P2 | one radio executor on `FROM_CPU` line 2: Wi-Fi driver, BLE lower link layer, the radio-system actor | every step bounded; long waits are timer waits |
| P1 / thread | role bodies, network stack, application | — |

- An interrupt handler never shares a lock with a task and reaches tasks
  through static typed signals, never through an executor's waker.
- Drivers never block or spin. A named settle of a known length up to 20 µs
  may busy-wait and counts against its context's step bound; a wait of
  unknown length is a timer-woken step with a deadline.
- A lower-priority task wakes a higher-priority one only through
  `oer_sync::PriorityWaker` ([notifications](execution-wakes.md)).
- Each interrupt table entry binds one interrupt part, whose type names its
  source, to one CPU line (16 to 47) at a level on a hart: the line's vector
  calls the entry's handler directly, the handler holds the part, and
  binding it returns the `Bound` proof a driver takes before it enables the
  peripheral's interrupt; a driver that polls a source no entry binds takes
  the router's `Silent` proof instead. The table fails to compile when two
  entries share a line or a source, and a binding when its entry lies
  outside the chip's interrupt controller.
- Each interrupt table entry declares its class: `Hardware` (capture,
  acknowledge, latch, raise a signal; never on its core's executor level),
  `Executor` (one per level and core; strict up to its task polls) or
  `Exempt` (a hardware handler that breaks one named rule, sitting on the
  executor's level or calling a waker, with its reason in the image report).
  The image check applies each class's rules; the level number grants
  nothing.
- Images have no reachable panics. The image check counts, for each root
  (the bootstrap, CPU0 as stage two's entry, each task, each interrupt
  context), the panic sites it reaches, by the crate layer of the function
  holding them: PAC, HAL, chips, portable, protocols, firmware, core,
  embassy, third-party, unnamed (`#[no_mangle]` and assembly
  symbols). Each image declares a budget of its runtime and bootstrap in
  `[package.metadata.open-radio.panic-budget]`, per layer and for the sites
  its `Hardware` interrupt contexts reach together (`hardware`); a count
  above the budget fails the build, and a budget only shrinks. The target of
  every budget is 0, first for the PAC layer and the `Hardware` contexts.
- Interrupt handlers and the executor code they run execute from SRAM or ROM.
  P2 tasks may run from PSRAM (the S31 reaches flash and PSRAM through
  separate controllers). Nothing runs from cached flash. The image check
  enforces it.
- Interrupt stacks have a bound, proven or conditional on an admitted
  assumption (the reports name it), and are sized from it at link time;
  task stacks are reported, and an image may declare them proven: a task
  stack whose image declares it proven fails the build on any unresolved path
  and is sized at link time to its bound plus its policy's minimum free
  bytes. The bootstrap jumps to a stage-two entry known at build time and
  the bootstrap stack's path ends there, once the checks verify the jump and
  that the entry installs its own stack pointer first. There is no baseline of accepted holes, and an observed depth above a
  bound fails the run. No frame
  exceeds the move limit (4 KiB); larger state lives in placed static
  storage and futures hold a reference. Code and stacks run from PSRAM after
  the staged bootstrap. Details: [memory and stacks](memory.md).

## Ownership

- One owner per resource, neither `Clone` nor `Copy`: `&mut` within a call, a
  token or lease beyond it, an actor across tasks. A handle shared over a
  mutex is not ownership. A read-only capability without side effects may be
  `Copy`.
- Who controls a state decides its form: the caller's own state is a
  typestate or token; the world's (air, hardware, peers, time) a typed
  refusal or outcome. A `Busy` the caller's own state explains is a design
  defect.
- A guarantee is written down only with what enforces it: a type with a
  compile-fail test, or a stated run-time check. A type-level mechanism names
  the bug it prevents.
- DMA-reachable storage is permanent and is released only after a proof that
  DMA stopped; otherwise it is quarantined until reset.
- A fatal path never takes over a block another owner holds: it keeps its
  record and resets the chip, and the next boot reports it. Its stop of the
  other hart (`StopTheWorld`: its hart masked, the other hart confirmed
  stopped by the hardware's stall status or held in reset) only prepares the
  reset; one compare-and-swap elects the fatal hart, so two failing harts
  never stall each other. A takeover needs a documented drain of the
  stopped hart's bus transactions after its stall, or one a stand run
  shows, and a revocation of the old owner's accesses.

## Hardware

- **One root at reset.** The S31 workspace contains no esp-* platform
  crates or esp-pacs. Host `espflash` and its transitive image-format parser
  serve image encoding and the stand. The reset root claims the chip once
  and splits its owned blocks by domain; no vendor singleton, adapter,
  handoff partition or shared-word inventory grants access. The bootstrap
  owns reset, its linker sections and application descriptor, and runs the
  bounded `oer-espressif-boot` sequences over the HAL's register traits.
  Stage two adopts the initialized platform without repeating cold boot.
  Its own traps, vectors, checked exits and the image's `critical-section`
  run on the typed interrupt controller. Each table entry moves its part
  through `Router::bind`; the executor receives owned software lines and
  timer blocks. Source identities come from the HAL's `ExecutorPlatform`
  contract, and architectural CSR access and waits from `oer-riscv`.
  `embassy-sync`, `embassy-futures`, `embassy-net` and `embassy-time` stay;
  tasks run on the static executor (`chips/espressif/executor`), in static
  slots, polled by direct calls, with one waker vtable whose data names the
  executor and task.
- **Register model.** The chip is block types (one hardware block each,
  offsets from its base) placed by a root at their addresses, with its
  interrupt sources, analog-I2C bytes (a separate space addressed by block
  and index), RAM regions and the memory the hardware reads and writes (DMA
  descriptors, the RX control prefix, TX descriptor words, BLE exchange
  memory), all in `chips/<chip>/registers/` with an ownership policy over
  them ([hardware memory](hardware-memory.md)). A bit is described, opaque
  (a use observed, meaning unknown), reserved or absent; only described and
  opaque bits are fields, each of one access class. Pointer fields state
  their target and encoding and take typed references; placement is proved
  by the linker; each block's handover protocol is its HAL engine.
- **PAC and HAL.** The PAC is the exact model, generated whole by `cargo
  registers generate`: typed blocks taken once by its root, field accessors
  by class, typed value domains and DMA pointers, interrupt sources,
  ownership parts, memory layouts, the analog-I2C dictionary and the few
  policy items accessors cannot express (whole-word images, masked
  acknowledgements, interrupt snapshots). It has no handwritten module and
  no logic. Register sets, phased sequencing, polls, retries, interrupt
  routing and lifecycle policy belong to the HAL, each vendor step with its
  `SOURCE:` ([PAC](../chips/esp32s31/pac/README.md),
  [the thin PAC](../tools/registers/model/README.md#the-thin-pac)).
- **Radio access.** The S31 Wi-Fi handoff returns one `RadioAccess` by value;
  the driver owns it through the runtime epoch and lends it with `&mut`. No
  shared register arena or dynamic borrow guard.

## Radio system

One actor on the P2 executor owns the shared RF front-end: PHY registration,
calibration and tracking, channel retune, modem clocks, coexistence timers
and sleep. Clients reach it through per-client cells; PHY membership is a
typestate (`Untracked`, `Tracked`, `Suspended`). It owns the HAL's
`RadioDomain` by value.

- **PHY.** Calibration and tracking are cut into bounded P2 steps: a ready P2
  peer waits for at most one hardware atom, no longer than 50 µs; with no
  ready peer, polling continues for at most 1 ms. A periodic tracking tick
  starts only if it overlaps no granted hard window and no announced retune,
  and is deferred by at most one period. A retune is one transaction; a
  failure after the first RF write poisons the PHY domain. Calibration
  matches the vendor when each field stays within the vendor's spread for
  that field, measured in the same run on the same board; the comparison
  runs when the calibration sources change. A change to how a calibration
  field is computed or replayed raises the calibration revision, so a cached
  record of another revision is never replayed.
- **Power and reset.** Common radio power enables the shared domain without
  resetting the Wi-Fi MAC. Wi-Fi resets its own baseband and MAC after PHY
  calibration at its client entry. Brownout policy belongs to the bootstrap.
- **Regulatory domain.** `oer-radio-regulatory` holds a validated domain (a
  generated subset of `wireless-regdb`) fixed for an installation; without
  one no radio installs. Every tune, install, scan and transmission of all
  three radios takes a `Permitted` channel. A no-initiating-radiation
  channel is listened on only; 5 GHz DFS channels are refused until radar
  detection exists. One transmit power ceiling (the domain's EIRP less the
  antenna gain, the chip's maximum, an owner cap) covers all three radios.
- **Coexistence.** Hybrid: hardware priority (PTI) and the vendor schedule
  serve ordinary traffic; typed air claims and grants serve hard deadlines
  only (the BLE connection event, 802.15.4 timed TX/RX and CSL slot, the
  beacon window of a dozing Wi-Fi station). Ports name what an operation is;
  one coexistence owner maps (client, kind) to a priority through the chip
  family's table (`oer-espressif-coex`). Each MAC's PTI registers belong to
  its driver. Windows shorter than a grant are enforced by PTI alone and
  reported as blackouts. The runtime that combines the policy and the
  schedule does not exist yet.
- **Low-power clock** is a type (`LpClock<RcSlow | Xtal32k>`).

## Radio ports

A port is the contract between a protocol's logic and a chip backend
([radio ports](radio-ports.md), [time](time.md)). `oer-radio-port` and the
protocol ports are not frozen; they grow with the protocols, each change with
its model and conformance case.

- Every port has a reference model in its own host crate,
  `protocols/<protocol>/model`; the hardware backend runs the same
  conformance cases. Frame protection is the `cipher` layer that backends
  and models take.
- One owner holds a lower-MAC core and its phase; every lifecycle transition
  is a method of a borrowed typestate view.
- A caller that drops an admitted request detaches only its waker; the port
  keeps the registration and buffer lease until the terminal event and
  reclaims exactly once. Event streams end with the uninstall.
- Abandonment is requested through a static typed signal and executed only
  in the driver's context, which erases keys and silences routes; a stop the
  driver does not confirm in its bound leaves the installation quarantined.
- No radio-system token crosses a port; PHY membership stays inside the
  backend. The Bluetooth LE port takes the form of the Wi-Fi lower-MAC port.

## Protocols

- **Wi-Fi** (`protocols/ieee80211`, driver `chips/esp32s31/ieee80211`). The
  driver owns the four EDCA queues and executes each attempt's retry plan;
  EDCA parameters belong to a VIF. Rate selection is its own portable
  machine: no fixed data or control-frame rate; frames before association and
  to a group use the lowest rate the BSS basic set allows. MSDU age starts at
  software admission. Monitor is a tap in the shared lower-MAC loop, not a
  role; ESP-NOW and FTM are clients of the lower-MAC port. A STA+AP pair keeps
  the access point running when the upstream cannot rejoin.
- **Bluetooth LE** (`protocols/bluetooth`, driver `chips/esp32s31/bluetooth`).
  HCI is the world boundary. Among conflicting connections the one closest to
  its supervision timeout wins; Host backpressure withholds acknowledgement
  instead of dropping the link. Controller memory is generated layouts with a
  HAL handover engine; no controller address reaches the driver. How much of
  an interval a connection event may take is policy of the portable link
  layer, from timing allowances the backend reports.
- **IEEE 802.15.4** (`protocols/ieee802154`). The port boundary follows the
  OpenThread radio platform: the backend executes acknowledgement, CSMA-CA
  and the cipher as advertised capabilities; retries, the pending-table
  policy, keys and counters stay in the core. Configuration is admitted in
  any radio state.

## Instrumentation and probes

`trace` (what happened), `profile` (what it costs) and `debug-control`
(knobs that force paths) are separate classes, each under its own cfg
(`oer_trace`, `oer_profile`, `oer_debug_control`) that only `cargo fw` sets
for an image declaring it. Probe hooks compile under `cfg(oer_probe)` for
`cargo fw probe` images only; production has no item only a probe uses.

## Tools

- **Images.** Only `cargo fw` builds images (`tools/image`); it runs the
  image checks (placement, stacks, frames) and `cargo fw sizes` compares
  builds.
- **Stand.** HIL runs (`cargo hil`, `tools/hil`) start by hand when they answer
  a real question. Stand roles belong to firmware, not boards; the lab router
  is reprogrammed to each run's profile; the stand file holds no credentials
  (each run generates and never logs them). A performance A/B is 5 of 5
  runs: a throughput regression above 5 %, a p95 latency regression above
  10 % or more CPU load for the same throughput blocks the change. Bisect on
  the stand is a manual `git bisect run` over one scenario.
- **Analysis.** Binary analysis is a set of building blocks (ELF view, RV32
  decoding, lifting, control-flow and value analysis, concrete execution)
  under `tools/analysis`, with Blobray as the entry point for vendor
  libraries; ELF is read only through `object`. Vendor register sequences
  are compared by hand with `tools/vendor`; a scenario prints its verdict and
  keeps nothing.

## Topic pages

[Repository layout](repository-layout.md) · [unsafe](unsafe.md) ·
[source policy](source-policy.md) · [hardware memory](hardware-memory.md) ·
[memory and stacks](memory.md) · [notifications](execution-wakes.md) ·
[radio ports](radio-ports.md) · [time](time.md) · [network](network.md) ·
[hardware errata](hardware-errata.md) · [protocol naming](protocol-naming.md)
