# Owner decisions

One line per decision the repository owner stated explicitly: date, question,
answer. Agents add a line only for such an answer, in the change it governs.
A later line that changes an earlier one says so.

## 2026-10-17: the repository starts

The owner and the architect session decided these point by point. The initial
tree is a selection of a predecessor repository, without its history.

- **Chips.** ESP32-S31 and ESP32-C5 are research targets and peers on the vendor SDK; ESP32-C6 follows later (owner, 2026-10-18).
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
  WPA2. IEEE 802.15.4 is the second block. After W4: W5 brings HE
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
- **Staged boot stays** (code and stacks in PSRAM). `tools/fw` builds, flashes
  and runs images with a minimal SRAM-placement check reported as warnings.
- **Execution-level DMA rules** are kept as facts and are the W1 checklist
  (`knowledge/esp32s31/wifi/dma-execution.md`).
- **No predecessor references.** The predecessor repository is private; this
  repository does not link to it or name it.

## 2026-10-18: the stand

- **Stand core.** The stand core of the predecessor's hardware-in-the-loop
  setup (the OpenWrt fixtures, the stand configuration, network and
  traffic) moves to `tools/lab`; its runner, on-board agent, evidence and
  archive do not. The stand router is a lab router: the tool brings it to
  the slice's profile and does not restore it.
- **W1 stand identity.** W1 identifies the stand's AP by the BSSID of the
  stand router's AP interface, read after the profile is applied, and
  checks the SSID from `[station]`.

## 2026-10-18: how the work runs

- **Review and merge.** A developer session opens a pull request and reports
  it to the architect session; the architect reviews it and merges it when CI
  is green, or returns it with remarks.
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
  form, ESP32-C5 crates, the facade) stays out.

## 2026-10-18: the RX descriptor walker

- **Owner bit.** Detached RX descriptors carry owner bit 31 clear: the
  walker stops on one instead of writing it (owner, 2026-10-18).

## 2026-10-18: hardware-shared memory

- **Memory the hardware reads and writes** (DMA descriptors, the RX control
  prefix, TX descriptor words, BLE exchange memory) is described in the
  register model as memory layouts with generated PAC accessors; the handover
  protocol stays in the HAL ([hardware memory](hardware-memory.md)).

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

## 2026-10-18: acceptance

- **Performance A/B:** 5 of 5 runs; a throughput regression above 5 % or a
  p95 latency regression above 10 % blocks the change (owner, 2026-10-18).
- **PHY calibration** matches the vendor when each field's difference stays
  within the vendor's own spread for that field (owner, 2026-10-18).

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
