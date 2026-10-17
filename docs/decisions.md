# Owner decisions

One line per decision the repository owner stated explicitly: date, question,
answer. Agents add a line only for such an answer, in the change it governs.
A later line that changes an earlier one says so.

## 2026-10-17: the repository starts

The owner and the architect session decided these point by point. The initial
tree is a selection of a predecessor repository, without its history.

- **Chip.** ESP32-S31 only; ESP32-C5 support is not carried.
- **What came over.** PAC, HAL, PHY (whole), the Wi-Fi driver's `dma`, `mac`
  and driver crates (`LowerMacCore` as a draft with known defects, below),
  the radio coexistence binding, base crates, the radio port, all protocols,
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
  WPA2. IEEE 802.15.4 is the second block.
- **Layout.** Code is placed by protocol and chip, not by layer (see
  `CLAUDE.md`); one firmware workspace; five packages are renamed to their new
  homes in a following change.
- **Hardware ownership root.** Two roots split by phase: esp-hal owns the SoC
  and everything before the radio handoff, our HAL the radio blocks after it.
  The handoff is a safe constructor consuming esp-hal's radio singletons
  (`I2C_ANA_MST`, `MODEM_SYSCON`, `MODEM_LPCON`, `LP_ANA`). Each shared word
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
- **Low-power clock** is a type (`LpClock<RcSlow | Xtal32k>`); the
  ESP32-S31-Function-CoreBoard-1 has no 32 kHz crystal.
- **Brownout** policy is applied in the bootstrap, not in a radio constructor.
- **Regulatory domain** is a validated type, mandatory for channel legality.
- **Licensing.** MIT OR Apache-2.0, ESP-IDF ports Apache-2.0 only with their
  modification notices; from sources without a license only facts.
