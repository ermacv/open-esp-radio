# ESP32-S31 HAL

Owners and hardware atoms over the thin PAC (docs/architecture.md,
"Layers"): the reset root and its partitions, typestate, register atoms,
fixed sourced vendor write series, interrupt routing, implementations of the
family drivers' register ports and of the platform contracts, and the DMA
memory core. A sequence that waits, polls, retries or decides on a read value
is driver code.

`root` claims the chip and splits its partitions. `platform` holds the chip
bindings: the hart geometry, core control, the interrupt controller and the
family platform contracts (`platform::Port`); `cores` holds the fatal path's
stop and the software reset. Ordinary blocks are `peripherals::<block>`:
`efuse`, `entropy`, `reset_cause`, `software_interrupt`, `system_timer`,
`timg0_alarm`, `usb_serial_jtag` and `watchdog`. `memory` is the DMA memory
core (`dma_address`, `dma_static`); the `dma_static!` macro is exported at
the crate root. `boot` holds the bootstrap's owners; the radio modules are
at the crate root.

Production files that wait, poll or run on deadlines are a per-file list
with exact marker counts in `oer-test-layers`
([layer tests](../../../tools/test/layers/README.md)): the Bluetooth
controller engine, scheduler and modem timer, the boot policy and waits in
`boot`, the Wi-Fi cold start in `ieee80211`, and the entropy read gap in
`peripherals/entropy.rs`. The list only shrinks; no
new file joins it. The scan does not find driver code without such a marker:
the IEEE 802.15.4 lifecycle policy is driver code that this crate still holds.
Shared radio membership, epoch policy, common power, semantic verification,
cold-baseline restore and clock refcounts live in `oer-espressif-radio`;
HAL supplies owned atom ports and affine proofs.
`oer-espressif-phy` owns analog-I2C power/reset readback decisions, retained-wake
parallel-host waits, PBus mode/completion order and short settles. PHY calibration
configuration/restore machines and other cross-block calibration operations
remain here; the no-wait scan does not classify them as atoms.

For BLE, `oer-espressif-bluetooth-engine` supplies the task/interrupt register
contracts, controller-time contracts, modify/stop progress values and
non-repeating installation identity. The existing HAL owners implement these
ports without transferring a P3 writer to P2. HAL retains affine publication
and release receipts, generative pool/epoch brands, arena projection and DMA
custody. The execution-lock/modify/stop machines, diagnostic settling,
request/reply exchange, terminal output release and role workers remain here.

For IEEE 802.15.4, `ieee802154::polled` retains task/IRQ owners with `Silent`
while exposing finite semantic transactions. `ieee802154::frames` owns the
permanent DMA static and projects each frame into the typed DMA window.
`oer-esp32s31-ieee802154-driver` implements the family engine's LL contract
and owns its service/placement wrapper above HAL. HAL has no normal or test
engine dependency.

`coex::CoexDomainPorts` binds the family coexistence policy
([`oer-espressif-coex`](../../espressif/coex/README.md)) to one radio domain:
it lends the timer bank, the event priorities and the clock to `CoexCore` as
its `CoexTimerHardware` and `CoexClockHardware` ports. The domain lends only
timers 0 through 4; timer 5 carries the PHY grant-protect request. The clock
port decodes the coexistence low-power clock observation with the 40 MHz
crystal of every supported board profile. The binding does not implement RF
grant notification.

`Root::into_bootstrap` returns unstarted owners without installing the console,
opening the timer clock or checking its latch. The family cold-start runner
performs those steps through the S31 boot binding. `into_runtime` also returns
unstarted owners without register access or installation. Its `RuntimePort`
implements the family boot adoption driver's owned adapters. That driver owns
clock adoption, installation order and watchdog arming; the compose binding
connects its `PlatformParts` to the staged runner. `PlatformParts` also serves
isolated verifier sessions, without a staged handoff or runtime dependency.
A ready staged `Runtime` exists only after driver arming and board/context adoption.
The split's `RuntimeCpuClock` exposes only a read/publish adoption port, with no
CPU-clock programming port. PSRAM controller, cache and MMU owners are retired.
Refused console and core-control installation keep their existing reason names.

The PSRAM driver owns the controller/MMU adapter and returns affine
`MappedPsram<PsramRegisters>`. Its physical window comes from the HAL adapter,
not a caller-supplied range. `PsramRegisters::into_region` consumes that exact
witness, retires remap authority and creates the one static mutable slice;
the HAL performs no PSRAM bring-up. Chip UI twins reject repeated loans,
remapping after a loan and foreign ranges or owners.

The watchdog owners retain both timer groups' clock/reset words. Runtime
startup selects XTAL and enables TIMG0 T0 through a private watchdog operation
that changes only `T0_SRC_SEL/T0_CLK_EN`; it preserves group reset, APB, T1 and
watchdog clocks. MWDT0 keeps that authority inside `StoppedMainWatchdog`.
`Hardware::timg0_alarm` adopts two disjoint owners without writes:
`Timg0AlarmProgram` owns T0 configuration, alarm, load and the whole ENA word;
`Timg0AlarmInterrupt` owns the whole RAW/ST words and only T0 CLR. Both lack
group-reset access. The program owner can travel in the actor's transaction
bundle: the actor can arm before depositing it, and the handler can rearm
while holding it.
Only the interrupt owner is bound permanently to the handler. They share no
software state or lock; each transition takes its own owner's `&mut`.

`arm_after` requires the IRQ owner's router `Bound` or `Silent` proof and a
positive relative microsecond interval within generated counter geometry.
It stops and masks T0, reloads zero, starts an increasing XTAL/divider40 one-shot
and unmasks delivery. It does not acknowledge an existing cause. `disarm`
only stops and masks; RAW remains for the handler's IRQ owner to observe and
acknowledge separately. The IRQ owner's `acknowledge` samples RAW/ST and
strobes only T0 CLR, leaving sibling causes, programming and ENA unchanged.
Zero and out-of-range input return `InvalidDelay` before MMIO. Separate
comparator observations at stopped count10000 cover equal/below targets;
zero-delay arming remains outside this API.

SOURCE for stop/load-zero/T0LOAD-strobe/start: ESP-IDF 4d80c1b2,
`components/esp_hal_timg/esp32s31/include/hal/timer_ll.h:236-239` (counter),
`:293-297` (reload value), `:319-322` (software reload).
SOURCE: `HW23_S31_TIMG0_RELOAD_20261027`,
`HW23_S31_TIMG0_COMPARATOR_20261027` and `HW7_S31_TIMG0_20261026` in
`registers/evidence/`: LOAD strobes reset both counter halves; rearm retains
pending. These facts do not establish exact comparator or IRQ latency.
The SAR/PWDET composition uses a permanent P2 `Exempt(ExecutorLevel)`
handler on line 16 below FROM_CPU2 line 17. Startup masks and clears stale
RAW; the actor arms before transferring the bundle and the handler acknowledges
before each step. Completion stops/masks before returning custody.
SOURCE: `HW23_S31_P2_ARBITRATION_20261027` qualifies only hart0 P2
line17-before16 pair; other pairs, levels and sustained fairness are unqualified.

## Platform clocks

The bootstrap's `boot::LpFastClock` implements the family's LP-fast selector
port. `oer-esp32s31-boot` generates the selection from `lp-fast-source` in
tracked chip/profile facts (currently XTAL) and supplies its sourced 3 µs
settle. `oer-espressif-boot::drivers::clock` writes the selector, holds under
the common boot deadline and rejects readback disagreement as
`ClockFailure::LpFastReadback` with both codes in the terminal boot report.
Every neighboring field of `ROOT_CLK_CONF` is preserved. The existing
`LpAonRadioClock` part moves into this bootstrap owner; runtime PHY receives
its part through the separate runtime root. RC_FAST has no startup profile.

`boot::BrownoutRegisters` implements the family's brownout port: a fixed
vendor write series, event enable, and individual word samples. Its owner
lends that port and its analog bus together. Threshold transaction, timeout
and readback policy live in `oer-espressif-boot::drivers::brownout`; the
chip's waits, thresholds and actions live in `oer-esp32s31-boot`.

`boot::Soc` owns the core-control, access-filter and TRNG parts and implements
`drivers::{cores,access_control,entropy}` ports. The family SoC-init driver
keeps containment → APM filters → TRNG and its single initial deadline check.
The chip profile supplies TRNG noise selectors and health-test settings.
`drivers::cores` also owns runtime status, park/run/stall confirmation and
fatal peer-stop/reset preparation. HAL `cores` retains the affine owner,
CAS election, `ClaimCell` quarantine, masked fatal proof and final system-reset
trigger. Its core-control port clears ROM entries but cannot publish a
secondary-hart launcher. Confirmation counts at most 10,000 completed status reads, with a fence
before sampling and after success, and no peripheral takeover authority.

`drivers::system_timer` owns latch settling and LO/HI/LO verification: 64
VALID polls per settle and 16 verification rounds, preserving the unverified
floor. HAL implements only strobe/VALID/LO/HI atoms. The executor reaches the
owned counter through `platform-port` and its binding requires the family's
latch port. The generic `Monotonic::start_initial_latch_check` explicitly
names the initial bounded check, before a `BootDeadline` exists. The reset
bootstrap binding calls it after opening the timer clock; the runner starts its common
deadline over the returned monotonic clock.

`power::ChipClocks` is the one owner of the platform clock words the radio
shares with the SoC (`MODEM_LPCON.CLK_CONF`, `HP_SYS_CLKRST.REF_160M_CTRL0`):
the family adoption driver installs it through the HAL adapter. Its
`oer-espressif-radio::clocks::ClockCounts` counts the references of every user;
the HAL retains the word claim and permanent `PsramMpll`. A refused acquire or
release is the caller's typed error.

Installation consumes the unique `ChipClockPeripherals` from the root by
value. These fixed-placement PAC owners cannot be cloned or recovered after
installation, so a second installation cannot be expressed. The paired UI
case `chip_clocks_installed_once` checks that consuming `Root` produces the
clock owner and prevents installing it again with that root.

- MPLL: the PSRAM mapping holds it for the whole image (`PsramMpll`), so a
  radio reference is a count and no path powers MPLL down (ESP-IDF 4d80c1b2
  `esp_clk_tree.c` `esp_clk_tree_mpll_acquire`; esp-hal 1f646c5
  `soc/esp32s31/clocks.rs:265-321`, PSRAM's never-released reference).
- BBPLL, the 160 MHz output's parent, stays powered for the whole image
  (the flash clock and the USB Serial/JTAG console run on it; ESP-IDF
  4d80c1b2 `esp_clk_tree.c:404-408`), so the 160 MHz edge is its gate only.
- The analog-I2C master's reset is not pulsed: no ESP32-S31 ESP-IDF path
  calls `regi2c_ctrl_ll_master_reset` (ESP-IDF 4d80c1b2
  `esp_hal_regi2c/esp32s31/include/hal/regi2c_ctrl_ll.h:43-53`).
- The Wi-Fi power gate opens once, at installation, and nothing closes it
  (esp-hal 1f646c5 `rtc_cntl/rtc/esp32s31.rs:670`, `enable_wifi_power_clock`).

## TSENS and peripheral SAR-I2C

The route-owned temperature resource tracks idle, acquired/restore-pending and
poisoned states. First acquire asserts SAR-I2C reset, completes its named 1 µs
settle, powers the bus, releases reset, then clocks/resets/enables TSENS.
Last release after RF close disables TSENS, its clock and peripheral power.
Pending calibration restoration prevents release; incomplete reset settling
poisons the owner and prevents neutral-root return. The sequence assigns no
literal zero to analog69.04. SOURCE: `phy/temperature.rs`, pinned ESP-IDF
`phy_override.c::phy_set_tsens_power`, `sar_tsens_ctrl.c`,
`sar_periph_ctrl_common.c` and S31 `regi2c_ctrl_ll.h`.

## Bluetooth register transactions

The interrupt bank clears and enables baseline images `0x8000` / `0x1302`,
including `CONTROLLER_TIME_OVERFLOW` in bank one. A primary status snapshot
carries this normal event separately from fault and dynamic scheduler sources.
CONTROL0 starts BTLC. CONTROL1/2 do not lower an enabled pending cause, so
terminal release masks the baseline, dynamic and sourced ISO/coex/CIG/CC
feature enable lanes in its handler mask step. The task's RUN-disable
step masks all task enables, including opaque task17, and strobes the sourced
all-lane task WT acknowledgement. Before the last visible fault check preceding
masking and before final release confirmation, the handler performs the same
affine NRT sample/sample/admit/acknowledge/acknowledge transaction as runtime.
Admission excludes shared primary fault and overflow clear lanes; a refused
pair stays unacknowledged and returns `ReleaseNotQuiet` with both exact snapshots
and the primary/task observations. Other sampled NRT lanes are acknowledged even
when outside the fixed cleanup images. Visible primary fault/unsupported causes
also return their complete observation for quarantine.
After RUN-disable, it writes fixed non-fault W1C images `0x18212103` / `0x207a`
under the mask, issues CONTROL1/2 and fences, then performs the admitted sampled
NRT acknowledgement before verifying zero primary/task enables.
Nonzero enables or residual visible status are `ReleaseNotQuiet`.
The fixed images exclude bank0[15] and bank1[8/9/12]. A fault arriving after
the last visible check is hidden and retained through release, so release
can succeed without reporting it. The next open's baseline W1C images
`0x8000` / `0x1302` silently discard that retained fault before enabling
its lane. This is a known limitation of the release/open transaction; there
is no workaround in this policy.
No enable is reopened and no matrix route is transferred. Masked pending is
not observable, so this proves neither zero pending nor producer shutdown.

SOURCE: `POLICY_S31_BLE_TERMINAL_NONFAULT_ACK_20261027`,
`BLOB_LIBBTDM_CONTROLLER_IRQ`, `BLOB_S31_BLE_IRQ_RELEASE_SCOPE_20261027`,
`BLOB_LIBBLE_ISO_COEX_REGISTERS`, `BLOB_S31_BLE_NRT_DISPOSITION_20261027`,
`HW32_S31_BT_MASKED_FIXED_W1C_20261027` and
`HW32_S31_BT_OUTPUT_OBSERVER_20261027` (PR #757).
Masked W1C is hardware-confirmed only for bank1 CC0/CC1/overflow.
Bank0/NRT and other feature lanes retain vendor coverage. Masked status cannot
prove zero pending; no positive fault-lane stimulus qualifies that conclusion.

The measured overflow is visible in primary status/source124 while the NRT
snapshots/source133 remain zero. Before either NRT acknowledgement in runtime
or release, the exact affine sample is checked against all primary-owned fault and
overflow clear lanes. `BluetoothNrtInterruptFault` retains both words and
refuses both writes when any such lane is present. An arrival absent from
the sample is absent from its clear image. This does not identify an NRT
producer or establish simultaneous set/clear priority.

The modem LP timer samples the low bytes of its pending and rollover words,
acknowledges each nonzero byte with a reviewed whole-word zero, and publishes
software pending with a whole-word one. These stores include the unmodeled
upper bytes. SOURCE: `BLOB_LIBBTDM_LP_PENDING_WORDS_CURRENT` (L4/L6/L7).
LP command lanes and mode controls retain their measured scope; compare disable
does not prove all-cause quiescence.

Source 127 must be serviced within one low-counter wrap (2^24 LP ticks).
`MODEM_LP_TIMER_SOURCE127_SERVICE_BOUND_XTAL_CYCLES` derives the interval
from the generated counter width and selected divider 399: 167.77216 s at
40 MHz XTAL / 100 kHz LP. Two natural wraps coalesce into one visible pending
indication; a zero-store after the second leaves no second visible credit.
Any future sleep path must wake on 127 before one wrap elapses. This is the
service contract; the constant adds no timer, wake programming or latency
measurement. SOURCE: `HW22_S31_LP_TWO_WRAPS_20261027`,
`ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK`.

Compare publication uses the typed 24-bit `COMPARE_TARGET` field, preserving
the opaque upper byte. Deadline projection and half-range checkpoints wrap
explicitly in that 24-bit domain. SOURCE: `HW22_S31_LP_COMPARE_WIDTH_20261027` and
`HW22_S31_LP_CONDITIONAL_ACK_20261027` qualify XTAL/divider1 observations
only. Production clears CONTROL0/4 and selects divider399; those observations
do not establish production timing or its latch witness.

Modem PLL source gating uses the pinned ESP-IDF 4d80c1b2 whole-word sequence:
open `0x15 → 0x1d`, close `0x2d → 0x25`. Platform source-clock readback
requires the open PLL image with the XTAL gate clear
(`ESP_IDF_4D59230D_S31_MODEM_PLL_GATE`).

Ordinary legacy, HT and HE queue preparation sets CONTROL bit 22. The
reserved-TXOP-slot adjustment is a known vendor difference, documented in
[the driver](../ieee80211/driver/README.md#tx-control-bit-22)
(`VENDOR_WIFI_TXOP_DESCRIPTOR_CLASS`).
FE `+0x8d0[21:20]` remains opaque: its nonzero sample contributes diagnostic
and RX-saturation observations, while estimator completion uses READY.

## Wi-Fi watchdog and interrupt evidence

Baseband initialization follows ROM `phy_bb_wdg_cfg`: it sets watchdog control
low16 to `0xaa` and control30, then enables `WIFI_BB+c40[31]`, the hardware
BB reset action on timeout. It preserves the independent interrupt-enable gate
at [30]; [29] is the timeout-clear request. The extent of the hardware BB reset
has not been measured. These fields do not establish a counter run gate or a
complete source123 interrupt/status/acknowledgement lifecycle.
SOURCE: `ROM_REV0_BB_WATCHDOG_LANES_20261026` in the register evidence.

Power source122 STATUS hides masked TSF timer0/1 events while RAW retains them;
a retained positive sample can clear them under the mask. A fresh zero STATUS
sample cannot clear hidden RAW. The masked clear-all transaction also clears
local timer0 pending. These stand facts do not qualify TBTT, beacon or opaque
lanes, nor simultaneous set/clear priority.
SOURCE: `HW8_S31_WIFI_TSF_MASK_20261026`.

`RadioHardware::split` consumes a permanently claimed `PhyIdentity` beside
its attachment. `RadioDomain::detach(operation)` returns an owned `RadioTransaction`:
the movable PHY half owns its register partition, restore state, registration
and grant-protect words; the inert return half retains common-power/clock
obligations and the attachment. A paired transaction reunites infallibly;
reuniting split halves checks their exact identity and returns both unchanged
on foreign input. Affine successful reunion leaves no stale return token.
Both payloads use `ManuallyDrop` until explicit reunion. Cancellation retains
them and reports `Cancelled(operation)` from either half. The HAL's
`phy_fault_reporter!` macro binds the image's direct link-time reporter.
Staged firmware retains operation/epoch and source observations in the common fault envelope and resets; hermetic host cancellation tests catch their own reporter.

`WifiClocked` and `RadioAccess` consume PHY custody into a
`ChannelTransaction`, which borrows their MAC and platform. Channel and retune
futures own that custody and return the domain with their typed result.


Bluetooth task and interrupt owners expose `interrupt_observation(&self)`
for value-only diagnostic reads. The interrupt owners return
all modeled primary enable/status and NRT words; the task owner returns its
complete enable word and the complete opaque +0x4bc word through a sourced
whole-word read; its unmodeled bits retain no event meaning. Dynamic
enables are subsets of the primary words. These sequential reads grant no
acknowledgement or release proof and do not establish a task cause/output
mapping. Comparison code may borrow the existing owner for these reads;
production uses `interrupt_enable_image` for task enables and never reads
+0x4bc. The primary observation remains a sequential, masked status view.
Before the first production enable write, `admit_interrupts` requires every
lane outside the typed production set to be zero: bank0 baseline15 and dynamic
21/27/28, bank1 baseline1/8/9/12 and dynamic3, and task14's scheduler lifecycle.
Task14 is acknowledged before RUN and at terminal stop; this policy supplies
no vendor scan-truncation runtime consumer. A foreign enable, including opaque
bank1[0,2] or task17, is `ForeignEnable` with seven source words
(primary ENA/status/NRT and task ENA).
Read side effects of the opaque +0x4bc view remain unestablished.
Its cited sources establish use, not general read safety.
`HW32_S31_BT_04BC_READ_SCOPE_20261027` covers the tested timer indications
without a positive task cause; general read safety remains unqualified.

Admission and terminal readback faults retain their owners and reach the
image's terminal policy. The returned fault retains the seven observation words.
Readback is sequential, not an atomic cross-bank sample or proof that every
producer is stopped. The stop sequence keeps its handler/task order.
SOURCE: `BLOB_S31_BLE_IRQ_RELEASE_SCOPE_20261027`,
`BLOB_S31_BLE_TASK_IRQ_INHERITANCE_20261027`,
`BLOB_S31_BT_TASK17_SELECTION_SCOPE_20261027`,
`HW25_S31_BLE_INHERITED_20261027`, `HW25_S31_BLE_RELEASE_20261027`.

The CTE interrupt bank at +0x4c4/+0x4c8/+0x4cc is outside this admission and
release policy. Production has no enabled CTE interrupt consumer; its
controller-global CTE publication is disabled. The adjacent bank is not a
primary ENA/status bank or the task +0x4b4/+0x4b8 bank. No all-bank cleanup or
pending-zero claim covers it (SOURCE: `BLOB_LIBBLE_PHY_INIT_REGISTERS` and
`BLOB_LIBBLE_HW_CTE_RING_CONTROL`).

Shared radio membership and registration epoch policy execute in
`oer-espressif-radio` over the S31 binding. The HAL retains affine power,
coexistence and registration proofs, platform clock guards, register owners,
restore storage, transaction seat/reunion checks and both quarantine guards.
BTBB and module membership, first/last common-power ordering and low-power /
external coexistence sequencing belong to the family driver. S31 modem settings
remain in `knowledge::modem_clock`.
