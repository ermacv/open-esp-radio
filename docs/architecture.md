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
platform             Espressif staged boot, interrupt table, panic record, linker layout
```

Wi-Fi is split into `wire` (over-the-air formats), `port` (the backend
contract), sans-IO cores and a `runtime`. Bluetooth LE has wire formats, a
backend port, synchronous LL/controller cores, an executor-independent
runtime and a reference model. IEEE 802.15.4 has `wire`, `port`, the `mac`
core and a separate host reference `model`. A backend depends on its
protocol's port, wire formats and implemented `cipher` only.

Protocol cores are **sans-IO**: frames, completed operations and the current
time enter as values; actions and the next deadline leave as values. They
never await, so they run under any executor and in host tests with virtual
time. Only a protocol's runtime awaits, and it binds no executor: time comes
from `oer-time`, interrupt signals from `portable/`, no executor,
`embassy-time`, `embassy-net` or HAL dependency (`embassy-sync` and
`embassy-futures` are allowed). The firmware composition declares the tasks
that poll the futures.
Drivers own hardware and wait; adapters bind executors.

The sans-IO safety and panic rules cover all crates under `protocols/` and
`portable/`, except the unsafe homes `oer-mmio`, `oer-riscv` and `oer-memory`.
They also cover the following Espressif crates: their ordinary dependency closures
contain no PAC, HAL, mmio or riscv crate, and their roots forbid unsafe code.
Build and development dependencies do not determine this membership.
Explicit crate-root `deny` of both `clippy::disallowed_methods` and
`clippy::disallowed_macros` marks a cleaned sans-IO crate whose local
`clippy.toml` contains the complete catalogue in
`tools/test/layers/sans-io-lints.toml`, the common root entries and its
existing local entries.

| Package | Directory |
| --- | --- |
| `oer-espressif-analog` | `chips/espressif/analog` |
| `oer-espressif-boot` | `chips/espressif/boot` |
| `oer-espressif-coex` | `chips/espressif/coex` |
| `oer-espressif-efuse` | `chips/espressif/efuse` |
| `oer-espressif-facts` | `chips/espressif/facts` |
| `oer-espressif-ieee80211-policy` | `chips/espressif/ieee80211-policy` |
| `oer-espressif-ieee802154-engine` | `chips/espressif/ieee802154-engine` |
| `oer-espressif-ieee802154-mac-values` | `chips/espressif/ieee802154-mac-values` |
| `oer-espressif-modem-clock` | `chips/espressif/modem-clock` |

The console `oer-espressif-usb-serial-jtag` depends transitively on mmio
through `oer-espressif-interrupt-table`, which owns platform interrupt
bindings below the unsafe boundary. Staged-layout owns an unsafe memory/ABI
contract;
staged-bootstrap, staged-runtime and executor depend on the platform.
These crates are outside the sans-IO layer. `oer-espressif-watchdog` is
also a platform owner: it owns hardware reset arming and the affine
system-deadline lease. `oer-time::deadline` contains only the deadline policy,
taking time observations and returning typed completion errors; it owns no
hardware or reset callback. The sans-IO rules forbid unsafe
code, runtime panic paths and dependencies on fatal, reset, retained-record,
HAL, PAC or mmio owners. Errors cross the boundary as typed values; the
image or HAL owner decides when to reset ([unsafe boundaries](unsafe.md)).

Code whose value is recovered knowledge or a pure algorithm is kept when it
moves; ownership, orchestration and wiring are rebuilt.

## Execution

The critical section is hart-local; staged build/run checks confine linked section
entry points to the image's declared hart roots. The public HAL cannot start or
release a secondary hart.

| Context | Runs | Rule |
| --- | --- | --- |
| P3, boot hart | radio interrupt handlers and each radio's composed `FROM_CPU` doorbell; the IEEE 802.15.4 engine (192 µs turnaround) | Wi-Fi and BLE handlers only latch status and post events |
| P2, boot hart, TIMG0 T0 CPU line 16 | SAR/PWDET conversion steps in `Exempt(ExecutorLevel)` | one bounded step per event; below FROM_CPU2 CPU line 17 |
| P2, boot hart | one radio executor on `FROM_CPU2`, CPU line 17: Wi-Fi driver, BLE lower link layer, the radio-system actor, including PHY calibration and tracking | cooperative admission between hardware atoms; long waits use timers |
| P1 / thread, boot hart | role bodies, network stack, application | — |

- `oer-memory::ExecutionContext` owns the unsafe memory-exclusion contract.
  `Entered<X>` is non-transferable, checks entry and rechecks each
  `ContextCell` access. `SignalExecutor` is safe and names this context;
  its signal payload and list use the same context type. Shared cells require
  a `Send` payload and a `Sync` sharing marker. Host signal constructors
  return a local owner (!Send/!Sync) and an atomic raiser (Send + Sync).
  The raiser only publishes facts; the owner takes or services payload on
  its constructing thread. A foreign-thread raise invokes no payload access
  or waker. Each executor mode and software line has its own typed list.
- An interrupt handler never shares a lock with a task and reaches tasks
  through static typed signals, never through an executor's waker.
- Each executor starts with a borrowed affine `Hart<H>`; its interrupt entry
  must name that hart at compile time. A software source identifies one
  executor globally, and its task wakers stay on its owning hart. Current
  execution state has one slot per facts-defined `HARTS`; its packed poll
  stamp reserves enough low bits for every hart and an unassigned sentinel.
  The remaining timestamp bits must hold the cooperative admission budget.
  `Doorbell::new::<Member>` returns its ringing owner and a zero-sized
  `UrgentSource<Member>` observation together, after checking that member
  against the software line and bound handler. Executor startup takes that
  typed observation, or `()` for no urgent source; callers cannot substitute
  an independently selected source number. Admission reads
  those sources and its own wake source from the current hart's pending bank;
  an idle wait checks that hart's debug status. These read-only selections use
  the PAC's checked hart identity; invalid raw IDs take the retained fatal path.
- Drivers never block or spin. A named settle of a known length up to 20 µs
  may busy-wait and counts against its context's step bound; a wait of
  unknown length is a timer-woken step with a deadline.
  PHY short busy-wait settles count `mcycle` through `oer-riscv`, at the stable CPU
  frequency the clock owner adopts from its register state during runtime
  handover. They require neither a ROM delay nor ROM clock calibration.
- PHY's shared transport and RX-gain drivers check CPU admission between
  complete hardware atoms. The contracts are a 50 µs atom and a 1 ms poll
  without a ready peer; admission stops at 900 µs to reserve the next atom
  and return path. These are cooperative contracts, not measured cutoffs:
  no check interrupts an overlong atom. Direct TXDC/PWDET paths do not query
  admission, and parent futures can chain immediately ready children.
  Neither the complete PHY poll nor the executor round has a universal
  1 ms cutoff. CPU admission does not admit an RF maintenance window
  ([PHY execution](../chips/esp32s31/phy/README.md#runtime-sequence-and-timing)).
- A lower-priority task wakes a higher-priority one only through
  `oer_sync::PriorityWaker` ([notifications](execution-wakes.md)).
- Each interrupt table entry binds one interrupt part, whose type names its
  sources, to one CPU line (16 to 47) at a named context's level on a hart: the line's vector
  calls the entry's handler directly, the handler holds the part, and
  binding it returns the `Bound<Entry>` proof carrying that entry's level and hart a driver takes before it enables the
  peripheral's interrupt; a driver that polls a source no entry binds takes
  the router's `Silent` proof instead. A part of several sources (a radio's
  interrupt banks and its doorbell) routes them all to its one line, and its
  handler learns which fired from the peripherals' status. A part is bound
  once and its handler holds it for the image's life: a task asks the
  handler for each bank transition, and the CPU route never changes.
  IEEE 802.15.4 prepares its IRQ owner before binding: while task code still
  owns both sides, it masks delivery, issues STOP and both timer stops and
  discards all fourteen stale event lanes without sampling. No pre-bind event
  can be serviced. The bindable IRQ owner is returned only after this step;
  bound activation then publishes the same ten runtime events as later
  sessions through the task owner alone. No task accesses the handler cell.
  Polling retains `Silent` after the same pre-bind discard.
  Binding returns a recoverable input refusal before placement. A failed
  admitted placement, handler installation or controller write returns a named
  fault with source observations, retaining its token and part in quarantine.
  The image records/resets through the existing retained slot, without
  publishing `Bound`, releasing owners or rolling back an unconfirmed write.
  Fault capture stores numeric reasons without reading cached text. Next-boot
  thread reporting decodes their names. The boot-zeroed claim preserves the
  current boot's first fault while allowing it to replace an unconsumed record
  from an older boot. Console and Wi-Fi input bind refusals terminate as image
  configuration faults through this same path.
  The table fails to compile when two entries share a line, or two entries or
  one part list a source twice, and a binding when its entry lies outside
  the chip's interrupt controller.
- `chips/espressif/interrupt-table` (`oer-espressif-interrupt-table`) owns
  the Espressif interrupt matrix contract, vector ABI and staged-runtime
  symbol names. Architectural CLIC line values live in `oer-riscv::clic`;
  chip facts select line geometry and MNLBITS. Signal task cells, atomic
  facts and intrusive lists live in `oer-memory::signal`. Chip crates and
  firmware use interrupt-table directly; portable support and protocol cores
  do not depend on it.
- A signal's constructor declares its receiving executor entry and all its
  raising contexts. An ordinary `FromHandler` is strictly above that executor,
  or the raiser is a task of a lower context; thread-mode tasks are at level
  zero. `FromExecutorLevel<Entry>` admits only an `Exempt(ExecutorLevel)`
  handler on the receiving executor's hart and level. Equal-level handlers
  cannot preempt the executor's round: the handler publishes atomic facts
  and pends the executor, whose next round observes the signal and wakes its
  task. The handler never calls the task's waker or touches task-owned state.
  The declaration checks the executor's software source, the same-level
  raiser's class, the directions and common hart.
  Doorbell construction, Wi-Fi activation and BLE gate exchange check that
  the request handler is above the requesting executor on the same hart;
  the driver also checks the doorbell against the bound handler. These are
  const checks over the table's entries, with no separate level-order proof
  and no runtime metadata in the signal or binding proof. `Doorbell<SWI, E, X>`
  retains both entries in its type and requires requester `X` to have class
  `Executor`; image wrappers derive their metadata from that type. Signal
  construction in static initializers is checked by `cargo check`. Generic
  runtime constructors and driver calls evaluate their inline const checks at
  monomorphization; paired codegen tests cover their rejection. A signal's
  `raise` call does not check its caller; the composition must keep calls in
  the contexts declared at construction.
- USB Serial/JTAG event enablement and endpoint construction require the
  sealed `EventAuthority` of the endpoint registers' associated interrupt
  part: its `Bound` or `Silent` proof. Missing authority and another part's
  proof fail to compile, including direct calls to the register trait.
- The CLIC facts select three implemented control bits and three level bits
  (eight levels, no priority bits). At equal level the highest line ID wins
  arbitration; equal-level handlers do not preempt one another. Image line
  assignments therefore define an explicit order within each level. The
  image declares its named contexts once and selects a context per entry.
  Entries at one level on one hart appear in
  increasing arbitration priority. The table checks their line IDs against
  the facts' highest-line tie-break. Inputs
  below the facts' first source line are reserved for core-local use. The HAL
  writes its selected MNLBITS on partition split without assuming the ROM
  left the reset configuration; the router checks INT_INFO and MNLBITS against
  the facts at install and verify.
  Stage-two adoption and interrupt enable return typed boot failures to the
  image's existing failure reporter when geometry, configuration, vectors,
  routes or lines disagree; they never enable interrupts on a mismatch.
- Each interrupt table entry declares its class: `Hardware` (capture,
  acknowledge, latch, raise a signal; never on its core's executor level),
  `Executor` (one per level and core; strict up to its task polls) or
  `Exempt` (a hardware handler that breaks one named rule, sitting on the
  executor's level or calling a waker, with its reason in the image report).
  The image check applies each class's rules; the level number grants
  nothing.
- Image code introduces no new runtime panics and removes existing ones.
  The image check counts, for each root
  (the bootstrap, each used hart's task entry, each task, each interrupt
  context), the panic sites it reaches, by the crate layer of the function
  holding them: PAC, HAL, chips, portable, protocols, firmware, core,
  embassy, third-party, unnamed (`#[no_mangle]` and assembly
  symbols). Each image declares a budget of its runtime and bootstrap in
  `[package.metadata.open-radio.panic-budget]`, per layer and for the sites
  its `Hardware` interrupt contexts reach together (`hardware`); a count
  above or below the budget produces a report with each site and its roots;
  image panic counts do not gate builds. Zero reachable panics remains the
  code goal: introduce no new runtime panics and remove existing ones.
  Compiler `panic_const_async_fn_resumed` sites have a separate row outside
  the budgets and target of 0, based on the executor's runtime `DONE` check
  (not a type guarantee) and `.await` never polling a completed child again.
- The w0, w1, w2, w3, b0, boot-smoke and radio-root images write diagnostic
  bytes directly, without `core::fmt` writer or argument dispatch. Host and
  test renderers may format values.
  Panic capture keeps static messages through `PanicMessage::as_str()`;
  parameterized messages are marked without rendering them. Next-boot panic
  output uses the existing binary diagnostic record.
  W3 retains `core::fmt::write` only in subtle/sec1 panic metadata on the
  SAE path; the RSN sans-IO boundary owns their conversion to typed errors.
- Interrupt handlers and the executor code they run execute from SRAM or ROM.
  P2 tasks may run from PSRAM (the S31 reaches flash and PSRAM through
  separate controllers). Nothing runs from cached flash. The image check
  enforces it.
- Interrupt stacks have a bound, proven or conditional on an admitted
  assumption (the reports name it), and are sized from it at link time;
  task stacks are reported, and an image may declare them proven: any
  unresolved path then fails the build, and each used task allocation is
  sized at link time to its bound plus its policy's minimum free bytes.
  Otherwise the boot hart's task bound may be partial, with unresolved sites
  reported; secondary task roots require complete bounds. Every hart has both
  stacks accounted for; contained harts link explicit zero-byte ranges. Stack
  sections, linker assertions, marker tables and vector coverage derive from
  the same fact geometry. The bootstrap jumps to a stage-two entry known at
  build time and the bootstrap stack's path ends there, once the checks verify
  the jump and that the entry installs its own stack pointer first. There is no
  baseline of accepted holes, and an observed depth above a bound fails the run.
  The frame/move limit (4 KiB) is a reporting threshold: larger frames and
  moves produce messages without failing the build. Proven task, interrupt
  and bootstrap stack bounds, unresolved-path restrictions and placement
  remain gates. Code and stacks run from PSRAM after
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
- An impossible state is a named fault retaining its source observations.
  Its owners stay quarantined while safe `fatal::reset_recording` retains
  the reason and resets; the next boot prints it. Watchdog expiry is not
  the fault transition.
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
  crates or bridges. Host `espflash` and its transitive image-format parser
  serve image encoding and the stand. The reset root claims the chip once
  through the PAC's single `take()`. The HAL's `Root::take` returns that
  unique owner; `Root::into_bootstrap` and `Root::into_runtime` split its
  partitions for each image's phase. Only these owned partitions grant
  access. The bootstrap owns reset, its linker sections and application
  descriptor, and runs the bounded `oer-espressif-boot` sequences over the
  HAL's register traits.
  All bootstrap code and ordinary constants stay in internal SRAM: cache
  suspension and flash tuning may reach out-of-line callees across crates.
  Every staged image build audits the complete linked bootstrap for this
  placement before encoding or flashing, including diagnostic runs. Only
  explicitly tagged image payloads and tuning references remain in flash.
  The PSRAM controller/MMU owner is consumed by its HAL bring-up method,
  which lends the mapped region once as a static mutable byte slice. Staging
  validates, copies and checks the image through that slice and retains its
  payload loan until the cache publication and stage-two jump.
  Flash tuning claims its SRAM scratch fallibly and checks each XIP word;
  a refused read carries its index and region length through `BootFailure`.
  All polling stays under the common boot deadline; failures use the
  bootstrap's existing terminal report. It writes bounded decimal observations
  directly, without formatter dispatch or another retained allocation.
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
- **Hart topology.** ESP32-S31's `[interrupts] harts` in
  `chips/esp32s31/platform/chip.toml` is the sole HP-hart cardinality;
  `boot-hart` is required and names primary initialization. Facts derive the
  host core count, vector geometry and the PAC's checked `Hart`,
  `SecondaryHart` and `HARTS`. Per-hart register and instance families take
  their counts from that list. Secondary boot-word labels also derive from
  its explicit boot role. Typed indices connect their geometry to HAL reads
  and writes. Fixed indices
  and list/count mismatches fail during the build. Every current image keeps
  secondary harts contained. The HAL holds secondary harts, and the image gate rejects
  starters, routes or storage for harts declared unused
  ([secondary boot](../chips/esp32s31/notes/core1-boot.md)).
  An architectural ID outside this list is `InvalidHart { raw }`, never a
  different hart. The fatal path records the full raw ID, quarantines free
  control owners and requests reset; it never takes a lent or held owner.
  The next boot reports the retained fault before clearing it; console
  refusal retains the record.
  The runtime's `Hart<H>` execution token and `PreAdmission<H>` stack-install
  authority are local, affine values minted from the boot entry's unique
  handoff proof. Adoption retains that authority until interrupt enable
  consumes it and returns the router with the same `Hart<H>`. Before enable,
  `Adopted::parts` lends the hart and mutable router together for executor
  setup. Facts generate one typed, aligned Rust vector-table row per
  hart, with assembly entry/exit and compiler-derived frame offsets.
  Generated secondary entry stubs admit no Rust code or interrupts and have
  no linked launcher.
- **Register model.** The chip is block types (one hardware block each,
  offsets from its base) placed by a root at their addresses, with its
  interrupt sources, analog-I2C bytes (a separate space addressed by block
  and index), RAM regions and the memory the hardware reads and writes (DMA
  descriptors, the RX control prefix, TX descriptor words, BLE exchange
  memory), all in `chips/<chip>/registers/`: `types/`, `layouts/` and
  `chip.toml`, with sources in `evidence/` and the schema-6 ownership policy
  in `policy.toml` ([hardware memory](hardware-memory.md)). A bit is
  described, opaque (a use observed, meaning unknown), reserved or absent;
  only described and opaque bits are fields, each of one access class.
  Pointer fields state their target and encoding and take typed references;
  placement is proved by the linker; each block's handover protocol is its
  HAL engine. Hardware codes and opcodes are sourced named field values in
  this model; sequence settings and images with unproved code meaning remain
  cited HAL configuration. HAL derives bounds, array lengths and wrapping
  moduli from generated geometry, including sourced block quantities outside
  registers.
- **PAC and HAL.** The PAC is the exact model, generated whole by `cargo
  registers generate --chip esp32s31`: typed blocks taken once by its root,
  field accessors by class, typed value domains and DMA pointers, interrupt
  sources, ownership parts, memory layouts, the analog-I2C dictionary and
  the few policy items accessors cannot express (whole-word images, masked
  acknowledgements, interrupt snapshots, pointer ORs). Its hardware API is
  generated; `src/lib.rs` exposes it and the single `take()` claim.
  Register sets, phased sequencing, polls, retries, interrupt routing and
  lifecycle policy belong to the HAL, each vendor step with its
  `SOURCE:` ([PAC](../chips/esp32s31/pac/README.md),
  [the thin PAC](../tools/registers/model/README.md#the-thin-pac)).
- **Radio access.** The HAL root returns one `RadioHardware` by value.
  Its split gives the radio system the shared `RadioDomain` and each
  protocol its partition. The Wi-Fi driver owns its `RadioAccess` through
  the runtime epoch and lends it with `&mut`. There is no shared register
  arena or dynamic borrow guard.

## Radio system

One actor on the P2 executor owns the shared RF front-end: PHY registration,
calibration and tracking, channel retune, modem clocks, coexistence timers
and sleep. Clients reach it through per-client cells; PHY membership is a
typestate (`Untracked`, `Tracked`, `Suspended`). It owns the HAL's
`RadioDomain` by value.

The radio system depends on client contracts, never on protocol drivers,
including through optional, target-specific or transitive normal dependencies.
`oer-esp32s31-radio-client::bluetooth::RadioController` defines the controller
lifecycle operations the actor invokes; the Bluetooth driver implements it.

- **PHY.** Constructing the actor does not calibrate. After the executor
  starts, the first client join calls `prepare` → `register_concurrent_phy`
  → `run_phy_register` in the P2 actor, including cold calibration or the
  admitted cache path. Initial client tracking settles pending work before
  operational use. While a tracked member exists, the actor drains requests,
  then waits on its doorbell versus the PHY tracking deadline. A timer win
  calls `track_concurrent_phy`, which evaluates demand and runs due work.
  The default period is 1 s, with eligibility at `previous + period + 1 µs`.
  SYSTIMER Alarm0 at P3 acknowledges and pends P2; the P2 executor dispatches
  timers and polls the actor. The direct maintenance TXDC/PWDET path submits
  each SAR/PWDET conversion to TIMG0 T0's P2 handler. Its explicit machine
  performs tone arm, the vendor's minimum 1 µs wait, SAR trigger, the minimum
  2 µs wait and timer-driven readiness samples. Edge steps return fixed relative
  1/2 µs intervals, reloaded after the hardware action without subtracting a
  later clock reading. A microsecond boundary cannot consume part of a minimum;
  interrupt latency and programming overhead can lengthen it. It judges ready before checking
  the 40 µs not-ready limit. It never polls a future. Other PHY leaves and the
  registration graph remain async.
  The actor arms before depositing an affine bundle into an `oer-memory`
  one-slot request cell. The bundle holds `PhyTransaction` (registers, restore
  slots, epoch and grant-protect state), `Timg0AlarmProgram`, its binding proof
  and the machine. The waiting future keeps `DetachedRadioDomain`; its drop
  guard therefore still records cancellation while the handler holds the PHY.
  The handler retains `Timg0AlarmInterrupt` for the image's lifetime,
  acknowledges RAW through CLR before every step, and returns custody through
  a one-slot reply cell and an `IrqSignal` declared with `FromExecutorLevel`.
  Startup stops/masks T0 and explicitly clears stale RAW before unmasking;
  rearming alone does not clear it. A pending event without a bundle is a named
  `SarAlarmWithoutBundle` fault retaining RAW/ST and request/reply lengths;
  `service` returns the fault in a `Result`, and the image keeps the owners
  quarantined while its direct retained reset call records the reason.
  Cancellation of the lifecycle future likewise retains any in-flight alarm until reset;
  terminal completion already stops/masks it before reply. The leaf's async
  suspension count is unchanged: a reply wait replaces its timer wait.
  `Client::sample` validates/registers its receiving context in one `poll_fn`
  and waits for the reply in another. The image uses the measured S31 hart0 P2
  pair, line 17 before 16 (HW23_S31_P2_ARBITRATION_20261027), and moves P1
  entries to lines 40–42 preserving their relative order. This observation
  qualifies only the P2 pair, not the other lines or levels. A higher-priority
  pending executor round runs first; there is no bound on starvation under
  continuously pending P2 executor work. Handler latency and RF quality with
  these event gaps are not measured.
  Registration, tracking, RF close and retained wake consume the domain and
  return it with their typed result. `RadioDomain::detach` owns a PHY
  transaction containing the registers, restore slots, registration epoch
  and grant-protect state. Its return owner retains the attachment and
  common power/clock obligations, without register access. Splitting that
  pair permits transfer; reunion accepts only the exact affine domain
  identity, independently of registration-epoch equality. Cancellation,
  including before the first poll, retains both payloads in quarantine and
  resets through a named retained fault; the next boot prints the reason.
  Channel selection and retune also hold an owned transaction across every
  wait; their future owns `ChannelTransaction`, whose channel HAL lends
  its registers within that custody. The retained
  cancellation record is 16 bytes and names the operation and epoch. The
  actor cannot retune while its domain is transferred. `admit_maintenance` checks pending tracking state; it has no hard
  RF-window calendar. The actor enforces no one-period maximum deferral or
  shift counter, and queued requests can postpone a tick. Grant-protect
  requests bracket RF-sensitive work without proving an exclusive RF grant.
  A retune is one transaction; a failure after the first RF write poisons the
  PHY domain. Calibration
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
  schedule does not exist yet. `RadioDomain::coex_ports` lends disjoint
  policy-timer and clock owners through exclusive borrows. The policy port
  cannot select reserved timer 5; each clock sample performs fresh reads.
  The family coexistence traits remain independent of these HAL owners.
- **Low-power clock** is a type (`LpClock<RcSlow | Xtal32k>`).

## Radio ports

A port is the contract between a protocol's logic and a chip backend
([radio ports](radio-ports.md), [time](time.md)). `oer-radio-port` and the
protocol ports are not frozen; they grow with the protocols, each change with
its model and conformance case.

- Queue splitting and initial body lending return typed construction errors.
  The hardware backend's P2 registration guard wraps admitted state and failed
  construction before publishing a frontend. Mandatory cancellation cleanup
  runs in that owner guard, even before its first poll. Portable lifecycle and
  slot code return typed failure observations and never invoke hardware cleanup.
  Failed construction retains its registered owner and source cause; it
  publishes no frontend or ordinary join. No missing resource is replaced with
  an empty or newly minted owner.
- Reference models are backends and use the port's conformance cases.
  Wi-Fi, Bluetooth LE and IEEE 802.15.4 models live in their protocol's
  `model` host crate. Frame protection
  lives in `cipher` where a software implementation exists; IEEE 802.15.4
  currently describes hardware offload through its port.
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
  driver owns the four EDCA queues and executes caller-selected single
  attempts, or a fixed-rate `ManagementTxPlan`;
  EDCA parameters belong to a VIF. Rate selection is its own portable
  machine: no fixed data or control-frame rate; frames before association and
  to a group use the lowest rate the BSS basic set allows. MSDU age starts at
  software admission. Monitor is a tap in the shared lower-MAC loop, not a
  role; ESP-NOW and FTM are clients of the lower-MAC port. A STA+AP pair keeps
  the access point running when the upstream cannot rejoin.
- **Bluetooth LE** (`protocols/bluetooth`, driver `chips/esp32s31/bluetooth`).
  Wire formats carry no protocol state or clock domain. The port owns radio
  vocabulary and the shared DTM window calculation; backends autonomously
  progress the admitted test with the Core Part F interval grid and 5 ms RX
  windows. Public backend identity crosses as an explicit wire value with
  named canonical/air byte order; the driver depends on neither LL nor HCI.
  HCI is the world boundary. The controller's DTM service owns correlation,
  command credit, pending commands, session leases and retained faulty terminals
  without IO. The LE runtime awaits capacity and terminals and runs the HCI
  service loop over composition-owned transport and port traits. Firmware owns
  framing, HAL entropy, installation/shutdown and task spawning. Among
  conflicting connections the one closest to its supervision timeout wins;
  Host backpressure withholds acknowledgement
  instead of dropping the link. Controller memory is generated layouts with a
  HAL handover engine; no controller address reaches the driver. How much of
  an interval a connection event may take is policy of the portable link
  layer, from timing allowances the backend reports. The permanently bound
  Bluetooth handler retains the modem LP timer's register owner and positional
  epoch while P2 processes the software queue. A value-only time snapshot goes
  to P2; an unarmed deadline plan returns through a single-slot queue. Only the
  handler performs rollover acknowledgement and the sourced compare-disable,
  counter/rollover sample, target or software-pending publication and bit-18
  command. During planning the handler acknowledges new pending/rollover facts
  and retains their software work until the in-flight plan returns. P2 shares
  no register writer or lock with the handler.
  The primary handler also acknowledges controller-time overflow and counts
  every observed wrap independently of scheduler-notification coalescing. P2
  brackets a live latch with that count and requests a post-latch handler
  service barrier before its second read. That entry counts hardware-pending
  overflow before P2 resumes. P2 retries a crossing transaction and joins the
  stable count and low raw word into continuous time. Sample gaps
  may span multiple raw periods; P3 still services each hardware wrap. Terminal
  output release masks and clears overflow before the bank leaves the handler.
  Admission checks primary/task enables before HAL/PHY initialization against
  the explicit production baseline, dynamic and task-lifecycle set; an enable
  outside that set is a named `ForeignEnable` fault. Release alternates handler
  and task steps, masks sourced feature lanes and task17, and acknowledges sampled
  NRT with the runtime transaction before the
  last visible fault check preceding masking and before final release confirmation.
  NRT admission refuses both writes for a pair containing a shared primary fault
  or overflow clear lane; release retains that exact pair in `ReleaseNotQuiet`.
  Other sampled NRT lanes are acknowledged even outside the fixed cleanup subset.
  Release writes fixed non-fault primary/NRT W1C images under the mask and
  acknowledges the task WT bank. Fault lanes are excluded from unconditional W1C,
  so arrivals after the check remain retained. Release verifies zero primary/task enables and rejects
  residual visible status as `ReleaseNotQuiet`. It never reopens ENA or changes
  matrix routing; masked pending is not observable and no zero-pending proof
  follows. Bank1 CC0/CC1/overflow have hw32 masked-W1C coverage; bank0/NRT
  and other feature lanes have vendor coverage, not hardware verification. The opaque task +0x4bc view is diagnostic-only and is never read by
  production. Both faults retain primary observations and task enables and
  quarantine owners through `fatal::reset_recording`; B0 reuses its 44-byte
  BLE gate slot. The separate CTE interrupt bank is outside this policy;
  production has no enabled CTE interrupt consumer. These sequential reads
  neither prove producer shutdown nor name opaque causes.
- **IEEE 802.15.4** (`protocols/ieee802154`, driver
  `chips/esp32s31/ieee802154/driver`). The S31 driver implements the family
  engine's LL contract over HAL's owned polling transactions and retains the
  engine with its placed DMA storage through detach/rebind. The family owner's
  borrowed `service` returns `Fault` with the complete source observation and
  entry state before acknowledgement or dispatch for unsupported samples. Supported
  samples retain their completed dispatch prefix, current state and typed invariant
  reason on failure, without replay or rollback. The S31 driver's consuming
  ISR entry quarantines the MAC and DMA owners without release or reuse;
  the engine owns neither quarantine nor retained/reset policy. PHY HIL owns the
  44-byte fault codec, retained slot and safe `fatal::reset_recording` call,
  reporting the previous fault on the next boot and retaining it on output refusal.
  HAL owns PAC access, startup discard, DMA allocation and projection; its
  normal dependency closure contains neither engine nor driver. Chip knowledge
  supplies hardware-free power facts; the driver composes the engine's typed
  level set. The port boundary follows the
  OpenThread radio platform: the backend executes acknowledgement, CSMA-CA
  and the cipher as advertised capabilities; retries, the pending-table
  policy, keys and counters stay in the core. The port owns shared pending
  tables, pending-bit selection, CSMA helpers and enhanced-ACK probing tables:
  the core configures their data and the backend executes the advertised
  offloads. CSL phase and network-time calculations stay in the core; wire
  codecs write ready IE values. Configuration is admitted in any radio state.

## Instrumentation and probes

`oer-trace` contains only event values, encoding, decoding and ordering of
caller-owned record slices. `oer-espressif-staged-runtime::trace` owns the
retained recorder, commit/clear protocol, global installation and previous-boot
policy. The image places its retained storage and recorder and supplies its
clock with `trace_clock!`. The platform preserves OETR layout 1: the six-word
header, 16-byte entries, three-word snapshot headers and snapshot payload words.
Retained storage uses plain atomic loads/stores; the recorder's read-modify-write
counters and global pointer/mask stay in ordinary atomic RAM.

`trace` (what happened), `profile` (what it costs) and `debug-control`
(knobs that force paths) are separate classes, each under its own cfg
(`oer_trace`, `oer_profile`, `oer_debug_control`) that only `cargo fw` sets
for an image declaring it. Comparison probe images (`cargo fw probe`) are built
over the production crates' public API alone. The separately admitted
`firmware/probes/esp32s31-facts` image takes the PAC once for short hardware
facts experiments, without HAL or production runtime dependencies. The HAL may
expose typed access to registers that production does not use, on the owner
that already holds them, so a comparison probe reaches them through the public
API. Such access grants nothing the owner's partition does not hold, and
production has no `cfg(oer_probe)`, probe constructor or probe module.
A comparison probe's observation needs do not justify generalizing driver
attachment, lifecycle or retirement APIs. Bluetooth retains the concrete
`ChipGate` for these paths; the additional typed register access stays on
the existing HAL owner.

## Tools

- **Images.** Only `cargo fw` builds images (`tools/image`); it runs the
  image checks (placement and stacks), reports frames and panics, and
  `cargo fw sizes` compares builds.
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
