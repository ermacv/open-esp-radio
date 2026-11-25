# ESP32-S31 HAL

Owners and hardware atoms over the thin PAC (docs/architecture.md,
"Layers"): the reset root and its partitions, typestate, register atoms,
fixed sourced vendor write series, interrupt routing, implementations of the
family drivers' register ports and of the platform contracts, and the DMA
memory core. A sequence that waits, polls, retries or decides on a read value
is driver code.

`root` claims the chip and splits its partitions. `platform` holds the chip
bindings: the hart geometry, core control, each hart's interrupt grant and
controller and the family platform contracts (`platform::Port`); `cores`
holds the fatal path's stop, the software reset and the secondary launch.

The CLIC is context-relative: each hart reaches its own at one address
(`HW1_S31_CLIC_20261026`), and the PAC claims its parts once per hart. The
root pairs each hart's CLIC parts with that hart's route bank in one
`InterruptGrant` (hart 0's in `InterruptBlocks`, hart 1's in the
`LaunchPermit`, which the launch hands to hart 1). A grant reads and writes
nothing until `into_controller` runs on its own hart (a `ForeignHart`
refusal otherwise); the controller (`InterruptController`, hart 1's
`Hart1Controller`) is `!Send`. The staged runtime calls it in each hart's
handoff. Hart 0's level bits are set when the root splits the partition on
the boot hart, hart 1's when hart 1 makes its controller. Each hart's
pending bytes lie in one static place that its grant holds for its
controller; the image reaches them only as the hart's `PendLines` (hart
0's in `Hardware`, hart 1's from its `LaunchPermit`), which gives the pend
handle of a line fixed at build (`LinePend`) and nothing else: no clear,
no index made at run time, no access to the part. `fatal_view`
observes hart 0's routes and lines in hart 0's fatal path only: hart 1's
routes have no claimed observer and its lines are reachable only from it. Ordinary blocks are `peripherals::<block>`:
`aes`, `cache_counters`, `efuse`, `entropy`, `reset_cause`, `software_interrupt`,
`system_timer`, `timg0_alarm`, `timg1_alarm`, `usb_serial_jtag` and `watchdog`. `system`
holds what lies below every block: the reset root (`root`), ownership
(`owner`), the harts (`cores`), the platform port (`platform`), the DMA memory
core (`memory`: `dma_address`, `dma_static`; the `dma_static!` macro is exported
at the crate root) and the bootstrap's owners (`boot`). `power` holds the
platform clocks, common radio power and the PMU sleep (`sleep`). `radio` holds the shared domain
(`domain`), the modem (`modem`), coexistence (`coex`), the PHY's registers
(`phy`) and Wi-Fi's hardware (`ieee80211`). The root drops the PAC's owners of their
blocks at its claim, as it drops the PMU wake observer, and the shared radio
drops the Bluetooth and IEEE 802.15.4 baseband words; the domain keeps no
BTBB membership and no Bluetooth low-power timer clock.

Production files that wait, poll or run on deadlines are a per-file list
with exact marker counts in `oer-tools-test-layers`
([layer tests](../../../tools/test/layers/README.md)): the boot policy and waits in
`system::boot`, the Wi-Fi cold start in `radio::ieee80211`, and the entropy read gap in
`peripherals/entropy.rs`. The list only shrinks; no
new file joins it. The scan does not find driver code without such a marker.

The Wi-Fi MAC's block reset, `RadioDomain::reset_wifi_mac_block`, is the
radio domain's owner's: it pulses `RST_WIFIMAC` in the shared reset word and
writes back the PHY's and the shared radio's words inside the block. Its
proof, `MacBlockReset`, is taken by the cold preparation
(`radio::ieee80211::init::initialize_wifi_mac`), which is the Wi-Fi owner's
and runs on the Wi-Fi partition alone.

Shared radio membership, epoch policy, common power, semantic verification,
cold-baseline restore and clock refcounts live in `oer-chips-family-radio-power`;
HAL supplies owned atom ports and affine proofs.
`oer-chips-family-radio-phy` owns analog-I2C power/reset readback decisions, retained-wake
parallel-host waits, serial-byte routing decisions and publication/completion
order, PBus mode/completion order and short settles. The generated S31 routing
dictionary stays behind `MasterAddress`, which cannot be constructed by a
caller or substituted with a fake port’s address. PHY calibration
configuration/restore machines and other cross-block calibration operations
remain here; the no-wait scan does not classify them as atoms.

`coex::CoexDomainPorts` binds the family coexistence policy
([`oer-chips-family-radio-coex`](../../family/radio/coex/README.md)) to one radio domain:
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
clock adoption, installation order and watchdog arming; the runtime binding
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

`Hardware::timg1_alarm` is the same pair for TIMG1 T0, with the same
operations, and `alarm_at`, which moves the alarm of the running counter to
an absolute target and enables it without a stop, reload or divider reset;
no product timer uses it. `Hardware::timg1_timer1` is TIMG1 T1's pair, with
`arm_after`, `alarm_at` and `disarm`: its program part holds T1's counter,
alarm and load words, and its interrupt part only T1's acknowledgement
strobe. The group's one enable word stays with T0's program part, which
opens T1's lane once (`open_timer1`) before T1's handler is bound; T1's
`disarm` stops its alarm instead of masking it, so no two owners write that
word. Runtime startup also selects XTAL and enables TIMG1 T0 and T1 in group
one's clock word, after the group's reset pulse.

`Hardware::cache_counters` owns the L1 cache's access counters of
instruction buses 0 and 1 and data buses 0 and 1 (the enable/clear word,
the overflow raw word and its clear strobes, every count, the L1 trace
enable they count under) and a read-only
observer of the configured cache and block sizes and the L2 bypass; no
product owner uses them. Adoption writes nothing. `start` turns the L1
trace on, stops every bus, clears the selected buses' counts and overflow
bits, then enables the selection in one write that sets no clear lane;
`snapshot` reads every count, then the overflow bits; `geometry`
decodes the one-hot size words and adds the fixed L1 ways. The L2 counters'
enables are debug-only in the vendor header and stay unclaimed.

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
port. `oer-chips-esp32s31-platform-bootstrap` generates the selection from `lp-fast-source` in
tracked chip/profile facts (currently XTAL) and supplies its sourced 3 µs
settle. `oer-chips-family-boot::drivers::clock` writes the selector and holds
the settle under the common boot deadline, as the vendor's
`rtc_clk_fast_src_set` writes and waits; it does not read the selector back.
Every neighboring field of `ROOT_CLK_CONF` is preserved. The existing
`LpAonRadioClock` part moves into this bootstrap owner; runtime PHY receives
its part through the separate runtime root. RC_FAST has no startup profile.

`boot::BrownoutRegisters` implements the family's brownout port: a fixed
vendor write series, event enable, and individual word samples. Its owner
lends that port and its analog bus together. Threshold transaction, timeout
and readback policy live in `oer-chips-family-boot::drivers::brownout`; the
chip's waits, thresholds and actions live in `oer-chips-esp32s31-platform-bootstrap`.

`boot::Soc` owns the core-control, access-filter and TRNG parts and implements
`drivers::{cores,access_control,entropy}` ports. The family SoC-init driver
keeps containment → APM filters → TRNG and its single initial deadline check.
The chip profile supplies TRNG noise selectors and health-test settings.
`drivers::cores` also owns runtime status, park/run/stall confirmation and
fatal peer-stop/reset preparation. HAL `cores` retains the affine owner,
CAS election, `ClaimCell` quarantine, masked fatal proof and final system-reset
trigger. Its containment port only clears ROM entries; the launch port
(`drivers::cores::release`, boot word last, and `retire_boot_address`) is
reached only through `Cores::start_secondary`, which consumes the runtime
root's one-shot `LaunchPermit`; the released hart's grant moves out once
(`Released::take_interrupts`). Confirmation counts at most 10,000 completed status reads, with a fence
before sampling and after success, and no peripheral takeover authority.

`oer-chips-family-drivers-system-timer` owns latch settling and LO/HI/LO verification: 64
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
`oer-chips-family-radio-power::clocks::ClockCounts` counts the references of every user;
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

The route-owned temperature resource tracks idle and acquired/restore-pending
states. First acquire asserts SAR-I2C reset, completes its named 1 µs settle
(`SAR_I2C_RESET_SETTLE`, a settle that cannot refuse), powers the bus, releases
reset, then clocks/resets/enables TSENS. Last release after RF close disables
TSENS, its clock and peripheral power. Pending calibration restoration prevents
release. The sequence assigns no
literal zero to analog69.04. SOURCE: `phy/temperature.rs`, pinned ESP-IDF
`phy_override.c::phy_set_tsens_power`, `sar_tsens_ctrl.c`,
`sar_periph_ctrl_common.c` and S31 `regi2c_ctrl_ll.h`.

## Modem PLL and transmit queue facts

Modem PLL source gating uses the pinned ESP-IDF 4d80c1b2 whole-word sequence:
open `0x15 → 0x1d`, close `0x2d → 0x25`. Platform source-clock readback
requires the open PLL image with the XTAL gate clear
(`ESP_IDF_4D59230D_S31_MODEM_PLL_GATE`).

Ordinary legacy, HT and HE queue preparation sets CONTROL bit 22. The
queue identity is `MacTxQueueIndex`, a closed four-value domain checked against
the generated ordinary-queue geometry. Raw input is admitted with `new`;
fixed identities use `constant`, checked during monomorphization. Bank selection
and event acknowledgement match that domain exhaustively.
The reserved-TXOP-slot adjustment is a known vendor difference, documented in
[the old driver](../radio/ieee80211/backend/README.md#tx-control-bit-22), a source outside the build
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

## Typical AES

`Hardware::aes`, transferred through the root's adoption resources, owns
the AES program and DMA IRQ parts and the complete
`HP_SYS_CLKRST.CRYPTO_CTRL0` word. `into_polled` consumes the router's
`Silent<AesInterrupt>` proof; `into_interrupting` instead hands the
`AesInterrupt` part to the image's router for a handler, which acknowledges
it (`AesInterrupt::clear`), and returns an `InterruptingAes` owner that may
enable the interrupt and runs DMA-AES (ECB, CBC, CTR) over the AES's AXI-DMA
channel 0 (`peripherals::aes_dma`: the channel, its placed descriptors and
three buffer pairs in the DMA window, no cache maintenance as internal SRAM
is outside the L1 cache; SOURCE: ESP_IDF_4D80C1B2_S31_AES_DMA). Both owners
expose the same Typical register atoms;
`oer-chips-esp32s31-drivers-aes` owns the sequence and waits. Clock/reset writes preserve
sibling and reserved bits; no fixed whole-word clock image is granted.
SOURCE: ESP_IDF_4D80C1B2_S31_AES and TRM_S31_V01_AES in the register model.
