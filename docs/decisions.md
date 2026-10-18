# Owner decisions

One line per decision the repository owner stated explicitly: date, question,
answer. Agents add a line only for such an answer, in the change it governs.
A later line that changes an earlier one says so.

## 2026-10-17: the repository starts

The owner and the architect session decided these point by point. The initial
tree is a selection of a predecessor repository, without its history.

- **Chips.** ESP32-S31 and ESP32-C5 are research targets and peers on the
  vendor SDK; ESP32-C6 follows later (owner, 2026-10-18). The ESP32-C5
  register model, HAL, platform and peers are carried in the predecessor port
  (owner, 2026-10-18).
- **What came over.** PAC, HAL, PHY (whole), the Wi-Fi driver's `dma`, `mac`
  and driver crates (`LowerMacCore` as a draft), the radio coexistence
  binding, base crates, the radio port, all protocols (known defects in the
  "Known defects" sections of their READMEs),
  the Wi-Fi runtime and policy, the staged-boot platform, the register model
  and generator, vendor pins and facts as data.
- **What did not.** The Bluetooth driver, the Wi-Fi execution form and its
  esp-hal adapter, a facade crate, trace, hardware-in-the-loop and stand
  tooling, qualification, verification code, binary analysis, image and
  device tooling, task runners, CI gates, skills and agent profiles.
- **Features.** Diagnostic features of the old test setup are removed;
  `validation-probes` and `validation-raw-dma` go in their own change right
  after the start; `test-support` product swaps are a separate task.
- **Panics** (clarified by the owner the same day). During active
  development our code may panic: `todo!` for an unwritten path,
  `unreachable!` for an invariant, `panic!` for unexpected hardware, `expect`
  instead of a bare `unwrap()`, each with a static message, since the panic
  record keeps only its start. Input from outside never panics: frames from
  the air, any peer value (counters, PNs, sequence numbers, lengths, indices),
  host and application input through an API; a wrong or impossible value is
  a handled error. The workspace warns on `unwrap_used`, `expect_used`,
  `panic`, `unreachable`, `indexing_slicing`; a crate moves to `deny` when its
  contract freezes: the `ieee80211/mac` and `rsn` parsers before W1, the PHY
  and radio system after R1, `lower-mac`, `sta`, `arbiter`, `runtime` and the
  Wi-Fi driver after W4, IEEE 802.15.4 and BLE when their slices freeze.
- **First vertical slice.** Wi-Fi station in steps, each with an observable
  hardware criterion: W0 boot, PHY and MAC up; W1 receive, passive scan; W2
  transmit, probe; W3 open association and ping on a hostapd access point; W4
  WPA2. Bluetooth LE is the second block (owner, 2026-10-18; it was IEEE
  802.15.4 before). After W4: W5 brings HE
  (HE-Control, trigger-based PPDUs, MU-EDCA, DCM, BSS colour for the access
  point, HE40); W6 brings throughput (TXOP holding and bursting, A-MPDU, the
  throughput gates of the acceptance catalog). External coexistence modes
  come with the coexistence block, Bluetooth low-power clocks with the
  Bluetooth LE block (owner, 2026-10-18).
- **Layout.** Code is placed by protocol and chip, not by layer (see
  `CLAUDE.md`); one firmware workspace; five packages are renamed to their new
  homes in a following change.
- **Hardware ownership root.** Two roots split by phase: esp-hal owns the SoC
  and everything before the radio handoff, our HAL the radio blocks after it.
  The handoff is a safe constructor consuming esp-hal's radio singletons
  (`I2C_ANA_MST`, `MODEM_SYSCON`, `MODEM_LPCON`, `WIFI`, `BT`, `IEEE802154`,
  `LP_TSENS`: the blocks the radio owns whole; `LP_ANA` dropped from the list,
  architect's clarification against the owned ranges: no radio range lies in
  it) and requiring the adopted PSRAM. Each shared word
  has one runtime writer (`registers/esp32s31/shared-words.toml`):
  `MODEM_LPCON.CLK_CONF` belongs to esp-hal and our HAL requests its gates
  through `clock::ll`; the other shared words belong to our HAL. The review is
  refreshed for the pinned esp-hal and rechecked at every fork merge with
  `cargo registers check shared-words`.
- **Radio system.** One actor on the P2 executor owns the shared RF front-end.
  Build its core — ownership split, actor with per-client cells, PHY
  membership typestate, retune transaction, `LpClock`, named bounds — in steps
  with the slice: R0 at W0; retune and unsliced tracking at W1–W2 with
  measurements; coexistence claims with the second block; doze and
  calibration slicing only when measurements require them. The 50 µs P2 step
  bound is knowingly exceeded during tracking ticks until then.
- **Port contract.** `oer-radio-port` and the lower-MAC port are unfrozen.
  W0–W2 use the chip driver API directly; that API is what the port backend
  wraps at W3. Earlier findings on the port are a checklist, reopened only with
  evidence. A type-level mechanism must name the bug it prevents or is
  proposed for simplification. Freeze after W4.
- **Unlicensed vendor sources.** No code or verbatim tables from
  `esp32s31-bt-lib`; their facts are in `knowledge/esp32s31/bluetooth/`, values
  to be measured on the stand. Bluetooth is rebuilt from the Core
  specification, Apache-2.0 Espressif sources, these facts and measurements.
- **Process.** Short engineering rules in `CLAUDE.md`; at most one AI review
  per pull request, on the owner's request, findings in the pull request; a
  branch and draft pull request per task, merged by the owner; one plain,
  non-blocking CI workflow; English in the repository.
- **Staged boot stays** (code and stacks in PSRAM). `tools/lib/service/firmware` builds, flashes
  and runs images with a minimal SRAM-placement check reported as warnings.
- **Execution-level DMA rules** are kept as facts and are the W1 checklist
  (`knowledge/esp32s31/wifi/dma-execution.md`).
- **No predecessor references.** The predecessor repository is private; this
  repository does not link to it or name it.

## 2026-10-18: the stand

- **Stand core.** The stand core of the predecessor's hardware-in-the-loop
  setup (the OpenWrt fixtures, the stand configuration, network and
  traffic) moves to `tools/lib/service/lab`; its runner, on-board agent, evidence and
  archive do not. The stand router is a lab router: the tool brings it to
  the slice's profile and does not restore it.
- **W1 stand identity.** W1 identifies the stand's AP by the BSSID of the
  stand router's AP interface, read after the profile is applied, and
  checks the SSID from `[station]`.

## 2026-10-18: how the work runs

- **Host tools.** One folder per CLI under `tools/cli`, libraries under
  `tools/lib` by layer (base, data, analysis, service); dependencies only
  downward, checked by a test (owner, 2026-10-18).
- **Review and merge.** A developer session opens its pull request as a
  draft at the start, pushes its work and marks it ready when it is done,
  which squash-merges it and deletes its branch; there is no review before
  the merge. Pull requests run no
  CI; CI checks main after every merge, and a red main is not a blocker: whoever
  broke it fixes it (owner, 2026-10-18).
- **Working rules.** The predecessor's working rules for agents return in
  `CLAUDE.md` (Work); its process bureaucracy (evidence bundles, seals,
  qualification, contract-change procedure, tracking-issue lists) does not.
- **What else returns from the predecessor, and when.** The PHY calibration
  cross-check against the vendor returns before PHY slicing (R1), which needs
  it to stay within the vendor spread. Binary analysis (Blobray, the RV32
  layers with the stack analyzer) returns in its own change at the first task
  that needs a vendor-code fact absent from `registers/` and `knowledge/`.
  The reset-retained trace returns when a failure cannot be explained from
  the log. The ESP-IDF reference peers return with the second radio block.
  The rest (verification harness, image and device tooling, the stand
  arbiter, repository tooling, the Bluetooth driver, the old Wi-Fi execution
  form, the facade) stays out.

## 2026-10-18: the RX descriptor walker

- **Owner bit.** Detached RX descriptors carry owner bit 31 clear: the
  walker stops on one instead of writing it (owner, 2026-10-18).

## 2026-10-18: hardware-shared memory

- **Memory the hardware reads and writes** (DMA descriptors, the RX control
  prefix, TX descriptor words, BLE exchange memory) is described in the
  register model as memory layouts with generated PAC accessors; the handover
  protocol stays in the HAL ([hardware memory](hardware-memory.md)).
- **Registers, layouts and protocol meet in the block's HAL engine**: pointer
  fields name their layout and take typed references; placement is proved by
  the linker, not checked at run time; `unsafe` stays in the PAC, the
  placement constructor and the protocol core, and crates above the HAL
  forbid it (owner, 2026-10-18).

## 2026-10-18: decisions recovered from the predecessor

Owner decisions that the predecessor recorded only in its issue comments,
confirmed by the owner again on this date. Policy values (intervals,
defaults, queue bounds) live in the code that applies them, not here.

- **Bluetooth Classic.** ACL first, later. One Bluetooth driver with LE and
  BR/EDR phases, a generic HCI layer with LE as one sub-dispatcher, and one
  air reservation for all of Bluetooth.
- **Bluetooth LE scope.** In time, every controller capability the hardware
  offers; multiple connections and the central role come first. The order of
  features does not matter; working, architecturally clean code does.
- **ESP-NOW and FTM** are clients of the lower-MAC port.
- **Access point width.** The access point supports 40 MHz in 2.4 GHz under
  the 20/40 coexistence rules; a station reports 20/40 OBSS as the standard
  requires.
- **Monitor** is a tap in the shared lower-MAC loop, not a role:
  station → monitor → station stays within one installation.
- **Behaviour on air needs the stand.** A claim about what the hardware puts
  on air (for example, the Duration that reserves the medium) is used only
  after a stand run confirms it; vendor evidence alone is not enough.
- **Group key with one slot.** A new key of any ID replaces the single entry
  atomically; frames under the old ID are dropped after the switch.
- **Coexistence by operation.** Ports name what an operation is (beacon, ACK,
  connection event, CSL slot, …); one coexistence owner maps (client, kind)
  to a priority through the chip family's data table.
- **Short hard windows.** Windows shorter than a grant are enforced by the
  hardware priority (PTI) alone and reported as an information-only
  blackout; rate selection ignores samples taken during an absence or a
  blackout.
- **Tracking ticks in free time.** A periodic PHY tracking tick starts only
  if it overlaps no granted hard window and no announced retune or resume; a
  tick is deferred by at most one tracking period, and every shift is
  counted.
- **No radio-system token at the lower-MAC port.** PHY membership stays
  inside the backend.
- **Priority and power ownership.** Each MAC's PTI registers belong to its own
  driver, and PTI values travel inside typed grants and notices. One transmit
  power policy, by board and country, covers all three radios.
- **Bluetooth LE port.** The Bluetooth LE port takes the same form as the
  Wi-Fi lower-MAC port.
- **Upstream projects are read-only.** No issues, pull requests, comments or
  reviews in upstream repositories; every change goes to the owner's fork,
  and how a fork follows upstream is the developer's choice.

## 2026-10-18: acceptance

- **Performance A/B:** 5 of 5 runs; a throughput regression above 5 % or a
  p95 latency regression above 10 % blocks the change (owner, 2026-10-18).
- **PHY calibration** matches the vendor when each field's difference stays
  within the vendor's own spread for that field (owner, 2026-10-18).

## 2026-10-18: portable time

- **Clock epoch offset.** Portable time starts at 2^31 µs: clocks offset
  their epoch so code at the boundary with 32-bit hardware counters meets
  the wrap at once (owner, 2026-10-18).

## 2026-10-18: stand runs

- **Manual, not mandatory.** Stand and HIL runs are started by hand when they
  answer a real question; no change requires one, no evidence is collected
  beyond a line in the pull request, and A/B runs only for a change that
  claims or risks performance. Automatic gates are considered only after the
  contracts stabilize. The stand tooling (boards, fixtures, power, runner,
  arbiter and a local status page in the browser) is built now, for manual
  use (owner, 2026-10-18).

## 2026-10-18: analysis and verification

- **Analysis platform.** Binary analysis returns as a set of building blocks
  (ELF view, RV32 decoding, lifting, control-flow and value analysis, the
  program model, concrete execution), not as one MMIO tool. Applications are
  built on them for vendor libraries and for our own images and runs: MMIO
  research, the stack analyzer, vendor comparison, provenance and copy
  checks, image checks, and more as needed.
- **Vendor comparison** (pins, facts, behaviour compared against the pinned
  vendor code) returns as a tool used by hand; its evidence index, shards,
  merge-base gate and qualification binding do not.
- **Probes stay non-invasive.** Production crates may carry macros that are
  active only in analysis builds and expand to nothing otherwise; product
  firmware contains no extra logic. Probes use stable public functions and
  methods. Every firmware image, probe images included, lives in the
  firmware directory (owner, 2026-10-18).
- **Vendor libraries in local and CI work.** Pinned vendor static libraries and
  ROMs, licensed or not, may be used in any form locally and in CI: analysed,
  executed in emulation, linked into comparison images. They are never
  published as our own, and no modified library is published; what the
  repository keeps from an unlicensed source is still facts only
  (owner, 2026-10-18).

## 2026-10-18: the Bluetooth LE code

- **Carried as it is.** The predecessor's Bluetooth LE driver, its controller
  memory, the S31 radio role, the controller's scheduling core and the HAL's
  scheduler locks are carried as they are, including what they took from
  `esp32s31-bt-lib`; this replaces the earlier rule that only facts from that
  library enter the repository (owner, 2026-10-18).

## 2026-10-18: decisions confirmed from the predecessor audit

The owner confirmed these again on this date; where the code differs, the
decision holds and the code is changed in the port's fix pass.

- **Dropped futures.** A caller that drops an admitted request detaches only
  its waker; the owner keeps the registration, queue position and buffer
  lease until the terminal event and reclaims exactly once.
- **Station TSF.** An update that would cross 2^64 is refused before any TSF
  write; a beacon whose TSF the station cannot follow is dropped whole; a
  join beacon whose TSF cannot be taken ends the association as an ordinary
  disconnect with a candidate refresh.
- **Interrupt code placement.** Code reachable from interrupts runs from SRAM
  or ROM only, checked by an image gate.
- **Stack bounds.** Interrupt stacks need a proven bound; task stacks accept
  any result with a warning; an observed depth above a bound fails the run;
  no baseline of accepted holes. A function leaves a call site's candidates
  only when every place that takes its address provably keeps its type.
- **HE trigger-based and beamformee** stay fail-closed until a hardware path
  is verified.
- **Port event streams** end with the uninstall; the next install starts
  with an empty queue.
- **STA+AP.** An upstream the pair cannot rejoin is reported as an event
  (`UpstreamNotJoined`) while the access point keeps running; the station
  reports the upstream's EDCA set as a fact and the pair applies it; the
  station's doze never closes the access point's transmit gate.
- **Keys and timeouts.** After an unconfirmed stop, secrets are erased by a
  required synchronous hook and the installation stays quarantined until
  reset; packet-number exhaustion is a typed port fault; no retry after a
  hardware TX timeout until compared with the vendor.
- **Wi-Fi MAC ownership.** The channel switch writes only the MAC control
  word and the REGDMA link select; TBTT events from the power interrupt are
  lossy, so the driver times its own TBTT wakes; coexistence values reach the
  driver by message.
- **RX Block Ack banks** form one pool for both roles, with separate role
  tables.
- **Bluetooth LE.** Among conflicting connections the one closest to its
  supervision timeout wins; HCI is the world boundary; an internal backend
  inconsistency is a radio fault that poisons the port; Host backpressure
  withholds acknowledgement instead of dropping the link.
- **No critical sections in portable protocols**: no `critical-section` or
  blocking mutex there, checked by the build with a reasoned allow-list.
- **Reference models.** Every port has a reference model that keeps its
  contract; doubles that break the contract stay out of the conformance
  suite.
- **Knowledge stays, shape goes.** Code whose value is recovered knowledge or
  a pure algorithm is kept; ownership, orchestration and wiring are rebuilt.
- **Compile time first.** Every check that can run at compile time runs
  there.
- **Stand.** Roles belong to firmware, not boards; each chip has a pinned
  recovery image; every scenario declares its air use; power loss is proved
  by the USB device leaving and returning.
- **MSDU age** starts at software admission, and every subframe keeps its
  own age through A-MPDU formation and retry; removing an aged head under an
  active Block Ack agreement sends a BlockAckReq.
- **Interrupt handlers** reach tasks through static typed signals, never
  through an executor's waker vtable.
- **EDCA** belongs to a VIF, not to the port.
- **IEEE 802.15.4 configuration** is admitted in any radio state, as in
  ESP-IDF.
- **Performance A/B** also measures per-core CPU load over the traffic
  window; more CPU for the same throughput blocks the change. The threshold
  is set with the owner at the first A/B.

## 2026-10-18: the predecessor's product code

- **Reference, not product.** The predecessor's Wi-Fi, Bluetooth LE and
  IEEE 802.15.4 orchestration (runtimes, roles, compositions, radio HIL
  families) was built on an ownership model the project left; it stays a
  read-only reference on the `port/archive` branch. The main branch takes
  its knowledge, slice by slice, and the hardware engines that do not depend
  on that model (the Bluetooth LE driver and controller memory, the
  IEEE 802.15.4 engine).
- **Lower-MAC port backend at W3.** The S31 backend of the lower-MAC port is
  designed at W3 together with a conformance suite that runs the same cases
  against the host model and the hardware; the port freezes after W4.

## 2026-10-18: package names

- **A package is named after its directory**, not after the predecessor's
  names: `crates/<path>` is `oer-<path>`, `firmware/<path>` is
  `oer-firmware-<path>`, a tool is `oer-tool-<path below tools/ without
  lib/>`, with no `[lib] name`. A clearer name moves the directory; the
  layers test checks the rule.

## 2026-10-18: the stand file

- **No credentials in the stand file.** Every access point and station of
  the stand is managed: a run generates the SSID and passphrase it sets on
  both, and never writes them to a log or the run store.
- **Only what cannot be found.** The stand file keeps what no tool can
  discover: the router and the air observer (SSH target, interfaces, radio),
  the Bluetooth adapter, the legacy BSS country, the hubs' port policy
  (`protected`, `switchable`, `keeps-power`) and, if wanted, the boards'
  human names. Hub ports, board chips and USB serial numbers are detected.

## Earlier decisions still in force

Made before the start; stated here because their records are not public.

- **Execution contexts.** P3 on core 0: radio interrupt handlers that only
  latch (the IEEE 802.15.4 engine owns its state there for its 192 µs
  turnaround) and one shared radio doorbell on `FROM_CPU` line 3. P2: one radio
  executor on `FROM_CPU` line 2 for the Wi-Fi driver, the BLE lower link layer
  and the radio-system actor. P1: role bodies, network stack, application.
  The level order is to be checked at compile time.
- **No shared lock** between an interrupt handler and a task; no blocking
  waits or spin loops in drivers; portable protocols never use
  `critical-section` or a blocking mutex.
- **Coexistence is hybrid.** Hardware PTI and the vendor schedule serve
  ordinary traffic; typed air claims and grants serve hard deadlines only;
  every port has a lost-air terminal.
- **Retune** runs `ic_mac_deinit → phy_change_channel → ic_mac_init` as one
  transaction; a failure after the first RF write poisons the PHY domain.
- **Wi-Fi driver core.** The driver owns all four EDCA queues and executes each
  attempt's retry plan (variant B); a protected MPDU is numbered at its first
  launch; DMA access only through its owner type, never alongside a
  quiescence proof.
- **Vendor parity is not a goal.** Clean architecture with hard boundaries,
  sans-IO protocols tested on the host and hardware offloads come first;
  vendor code is evidence of hardware behaviour, not a design to copy. A
  vendor algorithm may sit behind a seam as one policy but never shapes an
  interface.
- **PHY slicing.** Calibration and tracking are cut into bounded P2 steps:
  delays and readiness polls become timer-woken waits. Each phase states the
  per-step bound, the answer bound per request kind and the worst tracking
  tick, provisional until measured. Slicing keeps calibration results within
  the vendor spread. A section that cannot be sliced without leaving that
  spread goes to the owner with evidence and the option of running it on a
  core-1 executor; an agent never picks that option.
- **No fixed rates.** No fixed data or control-frame rate, not even
  temporarily: rate selection is its own portable machine built from the
  vendor policy.
- **Low-power clock** is a type (`LpClock<RcSlow | Xtal32k>`); the
  ESP32-S31-Function-CoreBoard-1 has no 32 kHz crystal.
- **Brownout** policy is applied in the bootstrap, not in a radio constructor.
- **Regulatory domain** is a validated type, mandatory for channel legality.
- **Licensing.** MIT OR Apache-2.0, ESP-IDF ports Apache-2.0 only with their
  modification notices; from sources without a license only facts.
