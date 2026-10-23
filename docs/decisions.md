# Decisions

The rules and choices below hold. A change adds or edits one only for an
answer the owner stated explicitly. Policy values (intervals, defaults, queue
bounds) live in the code that applies them.

## Scope

- **Chips.** ESP32-S31 (product) and ESP32-C5 (research target and peer on
  the vendor SDK).
- **Radios.** Wi-Fi in every mode, Bluetooth LE, IEEE 802.15.4, with hardware
  coexistence between them. Bluetooth Classic: ACL first; one Bluetooth driver
  with LE and BR/EDR phases, a generic HCI layer with LE as one
  sub-dispatcher, one air reservation for all of Bluetooth.
- **Bluetooth LE scope.** Every controller capability the hardware offers;
  multiple connections and the central role first. Working, architecturally
  clean code matters more than the order of features.
- **Vendor parity is not a goal.** Clean architecture with hard boundaries,
  sans-IO protocols tested on the host and hardware offloads come first;
  vendor code is evidence of hardware behaviour, not a design to copy. A
  vendor algorithm may sit behind a seam as one policy but never shapes an
  interface.
- **Licensing.** MIT OR Apache-2.0; ESP-IDF ports Apache-2.0 only, with their
  modification notices. Code written from the vendor libraries and ROMs
  (libphy, libpp, the ROM: PHY, the chips' PAC and HAL, station, Wi-Fi policy,
  coexistence) records facts, not a port, and stays MIT OR Apache-2.0.
- **Sources** ([source policy](source-policy.md)). From a source without a
  license only facts. The exception is the Bluetooth LE hardware engine (the
  S31 driver, the HAL's controller-memory engine, the PAC's Bluetooth
  initialization and the HAL's scheduler locks), which keeps what it takes
  from `esp32s31-bt-lib`.
- **Vendor libraries in local and CI work.** Pinned vendor static libraries
  and ROMs, licensed or not, may be used in any form locally and in CI:
  analysed, executed in emulation, linked into comparison images. They are
  never published as our own, and no modified library is published.
- **Other repositories.** Upstream projects are read-only: no issues, pull
  requests, comments or reviews there; changes go to the owner's forks, and
  how a fork follows upstream is the developer's choice. This repository
  names no other repository's issues, runs or commits.

## Code

- **Panics.** Input from outside never panics: frames from the air, any peer
  value (counters, PNs, sequence numbers, lengths, indices), host and
  application input through an API; a wrong or impossible value is a handled
  error. Elsewhere, during development, `todo!` for an unwritten path,
  `unreachable!` for an invariant, `panic!` for unexpected hardware and
  `expect` instead of a bare `unwrap()`, each with a static message. The
  workspace warns on `unwrap_used`, `expect_used`, `panic`, `unreachable`,
  `indexing_slicing`; a crate moves to `deny` when its contract freezes.
- **Air formats.** Fixed-width fields of frames (IEEE 802.11, Bluetooth LE,
  IEEE 802.15.4) are declared with one `macro_rules!` macro of the portable
  `oer-wire` crate, which generates checked accessors; frames are read and
  written through its bounded cursor, which never panics. Optional fields and
  element lists are code over that cursor. Each field names its clause of the
  standard. No hand-written masks or shifts for these fields. Formats the
  hardware defines (registers, descriptors, SRAM layouts) belong to the
  register model and its generator, which does not use `oer-wire`; a layout
  holding a standard's format gives only its place and size as bytes, and the
  protocol's `wire` parses it.
- **Compile time first.** Every check that can run at compile time runs
  there.
- **Knowledge stays, shape goes.** Code whose value is recovered knowledge or
  a pure algorithm is kept; ownership, orchestration and wiring are rebuilt.
- **No critical sections in portable protocols:** no `critical-section` or
  blocking mutex there; review enforces it, the build does not check it.
- **Hardware-shared memory** ([hardware memory](hardware-memory.md)). Memory
  the hardware reads and writes (DMA descriptors, the RX control prefix, TX
  descriptor words, BLE exchange memory) is described in the register model
  as memory layouts with generated PAC accessors. Pointer fields name their
  layout and take typed references; placement is proved by the linker, not
  checked at run time; the handover protocol is the block's HAL engine. A
  field the CPU writes in one phase and the hardware in another lists its
  phases with their writers.
- **Register bits.** A register or layout bit is described (named in an open
  vendor source or otherwise cited), opaque (a use observed, meaning unknown,
  cited), reserved (a cited source marks it reserved) or absent (no use
  observed). Reserved and absent bits have no field; a register records its
  reserved mask and source. A use is a write of a non-reset value, an explicit
  masked set, or a read whose value changes behaviour. Code preserves reserved
  and absent bits or writes their reset value; vendor comparison rejects a
  use of either and reports which. An initialization image the vendor writes
  to a whole register as an exact constant is kept as one cited image
  operation even where reset is unknown; its lanes without established
  meaning get no field.
- **Analog registers.** The PHY analog-I2C bytes are a separate register
  space addressed by (block, index), generated from the register model with
  the same bit states; no CPU address aliases them.
- **`unsafe`** stays in the PAC, the placement constructor and the protocol
  core of the HAL; crates above the HAL forbid it.
- **Probe hooks** compile under `cfg(oer_probe)`, which only `cargo fw probe`
  sets, for a probe image, in its own target directory; no crate has a
  probe feature. A probe uses the public API where it can; production has no
  item that only a probe uses. CI on main builds the probe images.
- **Instrumentation** is no probe, and its classes share no logic: `trace`
  (what happened: values and events), `profile` (what execution costs:
  time, cycles, load) and `debug-control` (knobs that force execution
  paths). Each compiles under its own cfg (`oer_trace`, `oer_profile`,
  `oer_debug_control`), which only `cargo fw` sets, for an image that
  declares the class in its manifest, in a target directory of its own; the
  classes have no Cargo features (`oer-trace` records under `oer_trace`).
  Main CI runs the host tests once per class cfg, of the packages whose
  sources hold code under it, found by search, each cfg in its own target
  directory. A further mechanism, such as a profiler, is discussed first.
  The lower-MAC conformance image declares `debug-control` for two holds:
  before publication and before completion delivery. Completion facts stay
  in the image's static P2 signal; the driver only prevents deadline service
  from consuming the held result. TX, DMA and air outcomes remain real.
  Working images w0–w2 remain byte-identical.
- **Every package builds**; the workspace has no `exclude` list.

## Execution

- **Contexts.** P3 on core 0: radio interrupt handlers that only latch (the
  IEEE 802.15.4 engine owns its state there for its 192 µs turnaround) and one
  shared radio doorbell on `FROM_CPU` line 3. P2: one radio executor on
  `FROM_CPU` line 2 for the Wi-Fi driver, the BLE lower link layer and the
  radio-system actor. P1: role bodies, network stack, application. The level
  order is to be checked at compile time.
- **No shared lock** between an interrupt handler and a task; no blocking
  waits or spin loops in drivers, with one exception: a named settle of a
  known length up to 20 µs may busy-wait, and it counts against the step
  bound of its context. A wait for an event of unknown duration (a
  handshake, a ring poll) is a timer-woken step with a deadline.
- **Task notifications across priorities.** A lower-priority task directly
  wakes a higher-priority task only through `oer_sync::PriorityWaker`, never through
  an AtomicWaker, Embassy Signal or another mutex-backed waker cell. Its
  consumer is serialized on one hart; it registers before checking the
  published input. See the [notification audit](execution-wakes.md).
- **Interrupt handlers** reach tasks through static typed signals, never
  through an executor's waker vtable.
- **Interrupt code placement.** Interrupt handlers and the executor code they
  run (entries, signal service and pend, the panic path) run from SRAM or ROM
  only. The tasks the P2 radio executor polls, and what they call, may run from
  PSRAM: the S31 reaches flash and PSRAM through separate MSPI controllers, so
  a flash operation never disables the PSRAM cache. Neither runs from cached
  flash. An image check enforces both; a diagnostic HIL image reports a violation
  instead of failing. The image linker places the strict code the analysis finds
  in cached memory into SRAM and links again; the image check verifies the result.
- **Stack bounds.** Interrupt stacks need a proven bound; task stacks accept
  any result with a warning; an observed depth above a bound fails the run;
  there is no baseline of accepted holes. A function leaves a call site's
  candidates only when every place that takes its address provably keeps its
  type. `cargo fw build` fails an app image, and so CI, when an interrupt stack
  has no proven bound, exceeds its budget or rests on an assumption other than
  the admitted ones; its analysis build is held to the same rule. A task
  stack whose image declares it proven fails the build on any unresolved path.
  Each hart's interrupt stack is sized at link time to its proven bound plus the contract margin (10 %), rounded up to 16 bytes, plus its guard; an image without a proven interrupt bound gets the contract's maximum.
  Each hart's exception path plus the largest frame on that hart fits the
  interrupt stack's guard, so an overflow the guard catches is recorded.
  The guard is per hart in the chip contract: 4096 bytes on S31 hart 0 and
  C5, 1024 bytes on S31 hart 1. Product images reject a guard violation.
  Every function's frame in an image is at most the move limit (4 KiB). App
  images fail and diagnostic images warn. An exception names the function, its
  reason and a ceiling it may not exceed.
- **Large state.** A value larger than the move limit (4 KiB) lives in placed
  static storage; futures and task arguments hold a reference to it. The image
  build denies a larger implicit move. A compiler check reports every value
  above the move limit that is not in static storage, with its source location.
- **Staged boot.** Code and stacks run from PSRAM after the staged bootstrap.

## Hardware ownership

- **Two roots split by phase.** esp-hal owns the SoC and everything before
  the radio handoff, except the system timer's comparators: the executor
  consumes esp-hal's `SYSTIMER` and writes them itself. Our HAL owns the
  radio blocks after the handoff, taken through a safe constructor consuming
  esp-hal's radio singletons (`I2C_ANA_MST`, `MODEM_SYSCON`, `MODEM_LPCON`,
  `WIFI`, `BT`, `IEEE802154`, `LP_TSENS`) and requiring the adopted PSRAM.
- **Shared words.** Each word that both esp-hal and our PAC write has one
  runtime writer (`chips/esp32s31/registers/shared-words.toml`):
  `MODEM_LPCON.CLK_CONF` belongs to esp-hal and our HAL requests its gates
  through `clock::ll`; the other shared words belong to our HAL. The review is
  rechecked at every fork merge with `cargo registers check shared-words`.
- **Temperature resources.** After `take_radio`, the HAL alone owns TSENS and
  peripheral SAR-I2C. Last release at RF close powers the peripheral bus down;
  first acquire asserts its reset, settles 1 µs, enables power and releases
  reset before TSENS clock/reset/enable. Initial esp-hal PMU setup precedes
  this owner and remains a disjoint lifecycle.
- **Radio system.** One actor on the P2 executor owns the shared RF front-end:
  ownership split, actor with per-client cells, PHY membership typestate,
  retune transaction, `LpClock`, named bounds.
- **Radio power and Wi-Fi reset.** Common radio power enables the shared domain
  without resetting the Wi-Fi MAC, on both initial and repeated entries. Wi-Fi
  performs its own cold baseband+MAC reset after PHY calibration at its client
  entry; IEEE 802.15.4 and BLE do not receive that reset.
- **Retune** runs `ic_mac_deinit → phy_change_channel → ic_mac_init` as one
  transaction; a failure after the first RF write poisons the PHY domain.
- **PHY slicing.** Calibration and tracking are cut into bounded P2 steps.
  A ready P2 peer waits for at most one atom, no longer than 50 µs. With no
  ready peers, polling may continue for at most 1 ms, as an accounting guard.
  Delays and readiness polls become timer-woken waits. Each phase states the
  per-step bound, the answer bound per request kind and the worst tracking
  tick, provisional until measured. Slicing keeps calibration results within
  the vendor spread. A section that cannot be sliced without leaving that
  spread goes to the owner with evidence and the option of running it on a
  core-1 executor; an agent never picks that option.
- **Tracking ticks in free time.** A periodic PHY tracking tick starts only if
  it overlaps no granted hard window and no announced retune or resume; a tick
  is deferred by at most one tracking period, and every shift is counted.
- **Low-power clock** is a type (`LpClock<RcSlow | Xtal32k>`); the
  ESP32-S31-Function-CoreBoard-1 has no 32 kHz crystal.
- **Brownout** policy is applied in the bootstrap, not in a radio constructor.
- **Regulatory domain** is a validated type, mandatory for channel legality:
  only the domain permits a channel, and every tune, install, scan and
  transmission of all three radios takes the permitted channel. Its rules
  are a generated subset of `wireless-regdb`. A domain is fixed for an
  installation: a country change is a reinstall; without a domain no radio
  installs. A channel without initiating radiation (NO_IR) is listened on
  only. 5 GHz DFS channels are refused until radar detection exists. An
  access point's Country element only narrows the domain for that
  association.
- **Priority and power.** Each MAC's PTI registers belong to its own driver,
  and PTI values travel inside typed grants and notices. One transmit power
  policy, by board and country, covers all three radios: the ceiling is the
  least of the domain's EIRP less the board's antenna gain, the chip's
  maximum and an owner cap; a per-attempt power only lowers it. The stand's
  10 dBm cap is a laboratory setting, not a product default.
- **RX descriptor walker.** Detached RX descriptors carry owner bit 31 clear:
  the walker stops on one instead of writing it. Execution-level DMA rules are
  facts in `chips/esp32s31/notes/wifi/dma-execution.md`.
- **Portable time** starts at 2^31 µs: clocks offset their epoch so code at
  the boundary with 32-bit hardware counters meets the wrap at once.

## Radio ports

- **Lower-MAC lifecycle.** One owner holds the lower-MAC core and its phase in place; every lifecycle transition is a method of a borrowed typestate view, shared by the port and any direct owner.
- **Port contract.** `oer-radio-port` and the lower-MAC port are not frozen.
  A type-level mechanism must name the bug it prevents. The Bluetooth LE port
  takes the same form as the Wi-Fi lower-MAC port. No radio-system token
  crosses the lower-MAC port; PHY membership stays inside the backend.
- **Station composition.** `StationPort` maps an image-owned composition to
  its concrete `Backend`, `SlotId` and `Caps`. The service retains the backend's
  lifecycle and body-loan identity; the mapping does not wrap or replace it.
- **Reference models.** Every port has a reference model that keeps its
  contract; doubles that break the contract stay out of the conformance
  suite. A port's reference model is its own host crate,
  `protocols/<protocol>/model`, beside the chip backends; frame protection is
  the `cipher` layer that backends and models take, never a core.
- **Dropped futures.** A caller that drops an admitted request detaches only
  its waker; the port keeps the registration, queue position and buffer lease
  until the terminal event and reclaims exactly once.
- **Abandonment execution.** The body requests abandonment immediately through
  a static typed signal: an atomic latch, an AtomicWaker and a pend of P2.
  The handoff shares no lock, `Cell` or `RefCell` between contexts.
  Only the driver context erases hardware keys, silences routes, abandons the
  receive session and detaches. The driver checks the request at the start of
  every poll, before any other work; after observing it there is no further
  publication, transmission or reception. A one-shot cell carries its answer.
  Async fail-stop, retirement expiry and ordinary stop wait within a named
  bound beside the retirement deadline, covering one driver step and the
  other ready P2 tasks. Only a timely driver proof yields `Stopped`; a missed
  bound yields `Unconfirmed` and quarantine. Owner Drop and cancellation of
  `run_port` only request and pend without waiting, retaining `Unconfirmed`
  quarantine. If the driver side is destroyed without another poll, its Drop
  erases keys and silences routes in P2; the execution adapter must establish
  that context and retain all borrowed backing until this Drop completes.
- **Event streams** end with the uninstall; the next install starts with an
  empty queue.
- **Coexistence is hybrid.** Hardware PTI and the vendor schedule serve
  ordinary traffic; typed air claims and grants serve hard deadlines only;
  every port has a lost-air terminal.
- **Coexistence by operation.** Ports name what an operation is (beacon, ACK,
  connection event, CSL slot, …); one coexistence owner maps (client, kind) to
  a priority through the chip family's data table. Each protocol's port owns
  its operation kinds; the shared radio vocabulary names only the clients.
  There is no urgency level. A row without a vendor source is a product
  choice that states its reason.
- **Hard deadlines** are the Bluetooth LE connection event, the 802.15.4
  timed transmission and reception and CSL slot, and the beacon window of a
  dozing Wi-Fi station; every other operation is served by the PTI alone.
- **Short hard windows.** Windows shorter than a grant are enforced by the
  hardware priority (PTI) alone and reported as an information-only blackout;
  rate selection ignores samples taken during an absence or a blackout.

## Wi-Fi

- **MAC preparation and start.** Cold Wi-Fi HAL prepares configuration and
  IRQs with common MAC gates closed; runtime start requires the prepared RX
  ring's affine DMA proof. Descriptor memory/binding preparation precedes
  common gates opening with MAC stop held, the sole BASE publication,
  walker enable and RX resume; each step requires its predecessor's proof.
  Promiscuous policy does not release MAC stop.
  W0 checks PHY up and MAC prepared with closed gates and no BASE; runtime
  MAC up belongs to W1/W2/R1.
- **Hardware lower-MAC conformance.** Run every Hardware case supported by
  the backend's declared capabilities. A-MPDU/BlockAck cases remain Hardware;
  a backend without `AmpduPhase` reports them separately as unsupported until
  W6, never as model-only. The gate e report lists the skips and their reason.
  Fixture body numbers are allocated with the body and released when it is
  freed; reusing an address after free is valid. Each operation also checks
  its original and returned body pointers independently.

- **Driver core.** The driver owns all four EDCA queues and executes each
  attempt's retry plan; a protected MPDU is numbered at its first launch; DMA
  access only through its owner type, never alongside a quiescence proof.
- **BSS-color collision** is an HE event, independent of TX queue collisions.
  The station masks it until its HE consumer exists; only a queue's
  `collision_pending` qualifies a TX collision detach.
- **MAC ownership.** The channel switch writes only the MAC control word and
  the REGDMA link select; TBTT events from the power interrupt are lossy, so
  the driver times its own TBTT wakes; coexistence values reach the driver by
  message.
- **EDCA** belongs to a VIF, not to the port. Each AC record carries AIFSN,
  ECWmin, ECWmax, TXOP limit and MSDU lifetime, defaulting to 500 TU;
  unsupported records are refused. One contention state per AC queue uses
  its head frame's VIF parameters. Before a BSS is known, a non-AP station
  uses the standard default EDCA set. TXOP limits are retained and validated;
  each access sends one frame.
- **MSDU age** starts at software admission, and every subframe keeps its own
  age through A-MPDU formation and retry; removing an aged head under an
  active Block Ack agreement sends a BlockAckReq from the port core. The
  backend reports expiry and never sends that request itself. Admission
  samples the image's monotonic clock at submit; a backend that needs a radio
  coordinate projects it through its `ClockRelation`. The VIF's per-AC lifetime
  defaults to 500 TU and is a typed duration; TU conversion is at the record
  boundary. Age is checked before publication and every retry or re-formation;
  an attempt owned by hardware is never aborted for age. Expired MSDUs have
  a separate terminal outcome and counter. The model takes the image's
  monotonic `oer_time::Clock`. Its radio clock is an affine `ClockRelation`
  with zero drift and a nonzero offset sampled at construction; `now()`
  returns that projection. Only the clock's owner advances model time.
- **Static TX profile.** Hardware backends consume a generic `BackendTxProfile`
  from port without dynamic dispatch. Algorithms, retry/BA/reorder state and
  tables belong to the cores and `ieee80211-policy`; the image/runtime composes
  the profile. The core supplies each fresh MPDU's PN through port; the backend
  formats it and reports BlockAck/RX values.
- **RX Block Ack banks** form one pool for both roles, with separate role
  tables.
- **Group key with one slot.** A new key of any ID replaces the single entry
  atomically; frames under the old ID are dropped after the switch.
- **Keys and timeouts.** After an unconfirmed stop, the driver erases secrets
  through the abandonment protocol in its own context; the installation
  stays quarantined until reset. Packet-number exhaustion is a typed refusal
  before backend handoff; no retry after a hardware TX timeout until compared
  with the vendor.
- **Station TSF.** An update that would cross 2^64 is refused before any TSF
  write; a beacon whose TSF the station cannot follow is dropped whole; a join
  beacon whose TSF cannot be taken ends the association as an ordinary
  disconnect with a candidate refresh.
- **STA+AP.** An upstream the pair cannot rejoin is reported as an event
  (`UpstreamNotJoined`) while the access point keeps running; the station
  reports the upstream's EDCA set as a fact and the pair applies it; the
  station's doze never closes the access point's transmit gate.
- **Access point width.** 40 MHz in 2.4 GHz under the 20/40 coexistence rules;
  a station reports 20/40 OBSS as the standard requires.
- **Monitor** is a tap in the shared lower-MAC loop, not a role: station →
  monitor → station stays within one installation.
- **ESP-NOW and FTM** are clients of the lower-MAC port.
- **HE trigger-based and beamformee** stay fail-closed until a hardware path
  is verified.
- **Standard before vendor.** What the standard defines (retry limits, the
  rate of management and group frames) follows the standard; a vendor value
  replaces it only with a stated reason. Data rate adaptation, which the
  standard leaves open, is built from the vendor policy.
- **No fixed rates.** No fixed data or control-frame rate, not even
  temporarily: rate selection is its own portable machine. Frames before
  association and to a group address use the lowest rate the band and the
  BSS basic rate set allow, every attempt at that rate, within
  `dot11ShortRetryLimit`; a control response takes its rate from the
  eliciting frame by the standard's rule. Both are derived, never constants.

## Bluetooth LE

- Among conflicting connections the one closest to its supervision timeout
  wins; HCI is the world boundary; an internal backend inconsistency is a
  radio fault that poisons the port; Host backpressure withholds
  acknowledgement instead of dropping the link.
- **Controller memory** is generated layouts: every word the controller
  shares is in the register model, a word or bits of unknown meaning as
  `_OPAQUE` fields, and the exact role images are constants of the generated
  types. Its handover engine is a HAL module that owns the controller's
  registers and its placed arena; role knowledge (images, list routing,
  priorities) lives in the driver; no controller address reaches the driver
  or above.
- **Connection-event span.** The backend reports its timing allowances
  (preparation lead, receive guard, widening jitter, minimum event duration)
  as capabilities; how much of an interval an event may take is the
  scheduling policy of the portable link layer, above the port. The vendor's
  span formula is evidence, not the policy.

## IEEE 802.15.4

- Configuration is admitted in any radio state, as in ESP-IDF, including an
  interface's settings while its client lives and its operation runs. The frame
  counter only moves forward, except when it is replaced together with the
  keys.
- A setting is in force once the call returns for addresses, keys, the
  source-match table and the ACK timeout; transmit power and CCA at the next
  transmission; receive-side values (promiscuous, coordinator, acknowledgement
  flags, pending mode) within one frame, at once while the radio listens with
  no frame in progress. Promiscuous reception acknowledges nothing and masks
  the acknowledgement settings without changing them. The backend keeps its
  settings across disable and enable.
- **Port boundary** follows the OpenThread radio platform: the backend
  executes, as advertised capabilities, the acknowledgement (frame pending
  from the source-match table, enhanced ACK from the CSL and Link Metrics
  parameters the core configured beforehand), CSMA-CA (backoff and CCA with
  the interface's macMinBE, macMaxBE and macMaxCSMABackoffs) and the cipher.
  Frame retries, the pending-table policy, keys and counters stay in the core;
  one publication is one transmission attempt. The backend owns the tables
  and parameters, and the core updates them through port requests, so the
  turnaround handler shares no state with a task.

## Stand and acceptance

- **Runs by hand.** Stand and HIL runs are started by hand when they answer a
  real question; no change requires one, and no evidence is collected beyond
  a line in the pull request. A/B runs only for a change that claims or risks
  performance.
- **Behaviour on air needs the stand.** A claim about what the hardware puts
  on air (for example, the Duration that reserves the medium) is used only
  after a stand run confirms it; vendor evidence alone is not enough.
- **Stand roles** belong to firmware, not boards; each chip has a pinned
  recovery image; every scenario declares its air use; power loss is proved by
  the USB device leaving and returning.
- **The lab router** is reprogrammed to each run's profile and not restored.
- **The stand file** holds no credentials: every access point and station of
  the stand is managed, a run generates the SSID and passphrase it sets on
  both and never writes them to a log or the run store. It keeps only what no
  tool can discover: the router and the air observer (SSH target, interfaces,
  radio), the Bluetooth adapter, the legacy BSS country, the hubs' port policy
  (`protected`, `switchable`, `keeps-power`) and, if wanted, the boards' human
  names.
- **Performance A/B:** 5 of 5 runs; a throughput regression above 5 %, a p95
  latency regression above 10 % or more per-core CPU load for the same
  throughput blocks the change.
- **PHY calibration** matches the vendor when each field's difference stays
  within the vendor's own spread for that field.
  Hardware measurement results that the vendor leaves unconsumed (the PHY
  CCA counter result words, whose window restarts at each Wi-Fi baseband
  reset release) are checked for readability, not compared by value.
- **Bisect** on the stand is a manual tool: each step builds its revision from
  a git worktree, runs one scenario and answers pass or fail to
  `git bisect run`.

## Analysis and verification

- **Analysis platform.** Binary analysis is a set of building blocks (ELF
  view, RV32 decoding, lifting, control-flow and value analysis, the program
  model, concrete execution) with applications on top: MMIO research, the
  stack analyzer, vendor comparison, provenance and copy checks, image checks.
- **Vendor comparison** is a tool used by hand for register sequences taken
  from the vendor: a scenario prints its verdict and keeps nothing. There are
  no point mutants, no evidence index and no coverage decisions; probe entries
  exist only for a scenario that selects them.
- **ELF** is read through `object`: convenient reading in
  `tools/analysis/elf`, bounded reading of untrusted vendor input in
  `tools/analysis/blobray-artifacts`; there is no other ELF parser.

## Repository and work

- **Layout** ([repository layout](repository-layout.md)): five roots by what
  the code is about: `protocols/` (sans-IO protocols and vocabularies),
  `portable/` (generic support), `chips/` (everything Espressif, the chips'
  registers, notes and vendor data included), `firmware/` (chip images) and
  `tools/` (the developer's machine, by domain, each domain with its
  libraries and its program). Dependency direction is not checked; names and
  the unsafe boundary are. Only `cargo fw` builds images.
- **Protocol layers.** Each radio protocol has `wire` (over-the-air formats),
  `port` (the backend contract), its sans-IO cores and a `runtime`; a backend
  depends on `port`, `wire` and `cipher` only.
- **Portable runtime.** Only a protocol's runtime awaits, and it binds no
  executor: time from `oer-time`, interrupt signals from `portable/`, no
  `embassy-executor`, `embassy-time`, `embassy-net` or HAL dependency;
  `embassy-sync` and `embassy-futures` are allowed. The firmware composition
  spawns its futures.
- **Names.** A library is `oer-` and its path below its root; an executable
  keeps its root (`oer-firmware-<path>`, `oer-tool-<path>`); no `[lib] name`.
- **Merge.** A pull request starts as a draft and is marked ready when its
  work is done; the owner squash-merges it and its branch is deleted; an
  agent never merges. Pull requests run no CI; CI checks main after every
  merge; a red main is fixed by whoever broke it.
- **Review.** At most one AI review per pull request, only when the owner
  asks for it; its findings go into the pull request.
- **Size budgets.** Growth of `.text`, `.rodata` or writable PSRAM, and of a
  type, future or frame, is reported in the pull request's `cargo fw sizes`
  table and never stops it; an image's code budget is reviewed when its milestone closes.
  Growth of an SRAM section, an allocated task stack or an interrupt stack,
  including type growth that grows one, stops the pull request until the
  owner answers. A size or step-cost reduction is tasked only when a hard
  limit or a timing contract (the PHY response bound) is reached.
- **Documents** state the current state. Plans, work lists and notes live
  outside the repository.
- **English** in the repository.
