# ESP32-S31 HAL

Register logic over the thin PAC (docs/architecture.md, "Layers"): the
reset root, sequences, polls, routing, clocks, power, resets and DMA
engines.

The reset root's `into_bootstrap` and `into_runtime` return `Result` with
`root::InitError`: a refused console installation, a stuck bootstrap
monotonic latch, or a refused stage-two core-control installation is a
named boot failure. Image callers handle it without a panic or fallback.

The watchdog owners retain both timer groups' clock/reset words. Runtime
startup selects XTAL and enables TIMG0 T0 through a private watchdog operation
that changes only `T0_SRC_SEL/T0_CLK_EN`; it preserves group reset, APB, T1 and
watchdog clocks. MWDT0 keeps that authority inside `StoppedMainWatchdog`.
`Runtime::timg0_alarm` adopts two disjoint owners without writes:
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
`HW23_S31_TIMG0_RELOAD_20261027` confirms this sequence on S31 at
XTAL/divider40 for 10/100/1000 µs and with pending retained across rearm.
LOAD resets both counter halves; programming LOADHI/LO alone does not.
Instrumented RAW observations agree with nominal 1 MHz counting; they do
not measure exact comparator or interrupt-entry latency.
`HW23_S31_TIMG0_COMPARATOR_20261027` observes no expiry at/below the stopped
count until T_EN is set, then prompt expiry; a running past target also fires.
`HW7_S31_TIMG0_20261026` in the register evidence supports one-shot expiry,
RAW retention under stop/mask, ENA redelivery and CLR at ENA=0/1, without a
clear/arrival collision or exact IRQ-latency guarantee.

The host fixture selects `Exempt(ExecutorLevel)` at P2 with a lower line ID
than FROM_CPU2. SOURCE: `chips/espressif/platform/family.toml` [interrupts],
RISC-V CLIC specification 3f8ee744, "Specifying Interrupt Priority";
ESP-IDF 4d80c1b2 `components/soc/esp32s31/include/soc/clic_reg.h` supplies the
CLIC geometry. SOURCE for priority/ID arbitration:
[ESP32-C5 TRM v1.1](https://www.espressif.com/sites/default/files/documentation/esp32-c5_technical_reference_manual_en.pdf),
section 2.8.2.5, p.86. `HW23_S31_P2_ARBITRATION_20261027` confirms line17
before16 on S31 hart0 at P2, with TIMG0/FROM_CPU2 assigned both ways and
both positively pending before admission. Other pairs, levels and sustained
fairness remain unmeasured. Nine images bind TIMG0 T0 for the production
SAR/PWDET pilot: W0–W3, B0, the Wi-Fi-station HIL profile, lower-MAC HIL,
PHY HIL and the RX-chain probe. Their permanent P2 `Exempt(ExecutorLevel)`
handler uses line 16 below FROM_CPU2 line 17. Startup stops/masks and CLR-clears
stale RAW; the actor arms before depositing the owned PHY bundle, and the
handler acknowledges before each step/rearm. Completion stops/masks T0 before
returning the bundle and raising the same-level reply signal.

## Platform clocks

`power::ChipClocks` is the one owner of the platform clock words the radio
shares with the SoC (`MODEM_LPCON.CLK_CONF`, `HP_SYS_CLKRST.REF_160M_CTRL0`):
`Root::into_runtime` installs it as `Runtime::clocks`, it counts the
references of every user, and a refused acquire or release is the caller's
typed error.

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

## Bluetooth register transactions

The interrupt bank clears and enables baseline images `0x8000` / `0x1302`,
including `CONTROLLER_TIME_OVERFLOW` in bank one. A primary status snapshot
carries this normal event separately from fault and dynamic scheduler sources.
CONTROL0 starts BTLC. CONTROL1/2 do not lower an enabled pending cause, so
terminal release masks the baseline, dynamic and sourced ISO/coex/CIG/CC
feature enable lanes in its existing handler mask step. The task's RUN-disable
step masks all task enables, including opaque task17, and strobes the sourced
all-lane task WT acknowledgement. Immediately before masking, the handler
checks visible primary fault/unsupported causes and the shared NRT clear
lanes. A visible fault returns its complete observation for quarantine.
After RUN-disable, it writes fixed non-fault W1C images `0x18212103` / `0x207a`
under the mask, issues CONTROL1/2 and verifies zero primary/task enables.
Nonzero enables or residual visible status are `ReleaseNotQuiet`.
The fixed images exclude bank0[15] and bank1[8/9/12]; a fault arriving after
the last visible check remains retained rather than being blindly cleared.
No enable is reopened and no matrix route is transferred. Masked pending is
not observable, so this proves neither zero pending nor producer shutdown.

SOURCE: `POLICY_S31_BLE_TERMINAL_NONFAULT_ACK_20261027`,
`BLOB_LIBBTDM_CONTROLLER_IRQ`, `BLOB_S31_BLE_IRQ_RELEASE_SCOPE_20261027`,
`BLOB_LIBBLE_ISO_COEX_REGISTERS` and `BLOB_S31_BLE_NRT_DISPOSITION_20261027`.
Masked W1C is hardware-confirmed only for bank1 CC0/CC1/overflow, run
`hw32-20261027-ble-masked-01` (#757). Bank0/NRT and the other feature lanes
have vendor coverage and are **not hardware-verified**. Opening a tested
lane with CPU routes at MAP0 still raises matrix124 and modem wake vector
`0x10`; terminal cleanup does not perform that transaction.

The measured overflow is visible in primary status/source124 while the NRT
snapshots/source133 remain zero. Before either runtime NRT acknowledgement,
the exact affine sample is checked against all primary-owned fault and
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
explicitly in that 24-bit domain. Each upper bit 24–31 and all together read
back zero and permit the future low24 compare in the short XTAL/divider 1
probe with CONTROL4=0 and either CONTROL0 state. The production low-power
prefix clears CONTROL0/4 and selects divider 399; its clock rate is not
reproduced by those probes. The conditional-ack race and surviving CONTROL2
latch were measured with CONTROL0=1/CONTROL4=0, XTAL/divider 1; they do not
establish the same latch witness or timing in production's configuration.
SOURCE: `HW22_S31_LP_COMPARE_WIDTH_20261027`,
`HW22_S31_LP_CONDITIONAL_ACK_20261027`.

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
Staged firmware retains a 16-byte operation/epoch record and resets; hermetic host cancellation tests catch their own reporter.

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
Its cited sources establish use, not read safety, and HW25 did not read it
on the board. hw32 preserved the three tested bank1 indications and BTLC
progress across reads, but had no positive task indication; general read
safety remains unqualified.

Admission and terminal readback faults retain their owners and reach the
image's retained reset path. B0 stores these seven words and a reserved zero
word in its existing 44-byte BLE gate record with a reason-tagged payload; the next boot prints them.
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
