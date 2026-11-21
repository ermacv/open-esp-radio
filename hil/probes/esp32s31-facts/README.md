# ESP32-S31 hardware facts image

This PAC-only image owns the chip once and executes short, bounded register
experiments on hart 0. Hart 1 is held in global reset with its CPU clock gated
before a nonzero boot address is written; only selections 232/233 and 323
release it for one bounded entry observation and park it again. There is no HAL dependency and no
production probe API. `src/image.rs::PROBES` is the single registration point.

The experiments that drive or observe the Bluetooth LE or IEEE 802.15.4
blocks (the bt-mac, its low-power timer and ETM channels, the 802.15.4 MAC)
are out of the build with their radios. Their modules (`ble`,
`ble_inherited`, `ble_masked`, `ble_wave2`, `lp_ack_race`, `lp_lanes`,
`lp_softreset`, `pb1`) are not declared, and the 802.15.4 functions of
`radio_local` lie in its undeclared files `ieee802154.rs`,
`hw30_ieee802154.rs` and `wave2_ieee802154.rs`. Their selections (20–35,
80–82, 90–114, 170–172, 180–183, 210–219, 223, 224, 230, 231, 240–248, 269,
300–302) answer as unknown: record `0xed` with the selection, marker
`0xbad00004`, halt. [OTHER-RADIOS.md](OTHER-RADIOS.md) keeps their
descriptions, and [BTLC.md](BTLC.md) the contract of 230 and 231.

Build with `cargo fw probe oer-hil-probes-esp32s31-facts`. Load the ELF
into SRAM with the ROM loader under a `cargo stand lease --board s31-a --air none`.
The entry owns its stack, clears BSS and preserves its LP `.noinit` record.
The observed ROM-download state rejects direct JTAG execution from SRAM with
PMP instruction access fault. Use the standard ROM MEM_END handoff after the
stand's download-mode reset, for example within the held lease:

```console
espflash flash --ram --ignore-app-descriptor --no-stub \
  --before no-reset --after no-reset --chip esp32s31 \
  --port <leased-port> <probe-elf>
```
 `--ram` loads the ELF; it does not write flash.
Image compilation still belongs exclusively to `cargo fw probe`.
The JTAG session must select only HP0 (`ESP_ONLYCPU=1`), suppress OpenOCD's automatic
watchdog MMIO writes, and must not issue a debugger reset between the software
reset experiment and its next-boot observation. Peripheral accesses belong to
the probe's typed PAC owners; the debugger loads code and reads result memory.

The initial marker is `0xfac70000` (ready). Publish `FACTS_REQUEST` in SRAM
while halted, then resume the same context. The session keeps its one PAC claim
across commands; do not reload the ELF or re-enter `_start` to select a leaf.
A valid retained reason automatically selects the next-boot observer on a new
ROM-loader entry after software reset, without another reset. Core and reset
selections:

| Selection | Experiment |
| --- | --- |
| 0 | Read-before-write CLIC geometry/configuration, held-hart boot address, edge pending, threshold, then five latency samples |
| `u32::MAX` | Idle in WFI awaiting a debugger command |
| 1 | One software edge-pending publication |
| 2 | CLIC implemented fields and all line fields before configuration |
| 3 | MNLBITS/NVBITS/NMBITS before configuration |
| 4 | Three threshold comparisons: below, equal, above |
| 5 | Five FROM_CPU2 → current runtime trap-entry cycle samples |
| 6 | Full-width boot-address writes/readback while hart1 is held |
| 7 | UART restore, other-core reset preparation, LP reason publication, system reset |
| 8 | Retained reason, UART and reset-cause observation after that reset |
| 9 | Raise-to-ceiling race for FACTS_THRESHOLD_K (0–24): eight NOP/readback variants, three repeats each |

`FACTS_OUTPUT` has 514 little-endian words: completion marker, record count,
then four-word records `(tag, a, b, c)`. Marker `0xfac70002` means running, `0xfac70001` means completed;
`0xfac70007` precedes system reset; `0xbad00001..5` denotes an interrupt storm,
exception, panic, unknown selection or result-buffer exhaustion. Completion
means the leaf finished, not that its hardware claim passed. SRAM output after
system reset is not assumed retained.
The reset leaf publishes twelve LP words, including the pre-trigger UART/hold
observations and the reason tag. The tag identifies the leaf and sample. The handler stops after eight unexpected entries;
ordinary observation windows are 4096 cycles. Completion idles in WFI until
the bounded debugger capture halts the target.

The interrupt entry/exit assembly is included from the current staged runtime
and its vector aliases call the probe handler directly. The task stack is SRAM,
not production PSRAM; the image does not reproduce scheduling, load, cache
contention or a running product's clocks. Cycle samples characterize this
isolated entry path, not a worst-case latency guarantee.

Selection 9 starts FROM_CPU2 inactive at CTL 0x1f and threshold zero.
While halted, publish `FACTS_THRESHOLD_K` (0–24) before `FACTS_REQUEST=9`.
Each entry enables MIE, calls a typed PAC store thunk without readback,
executes K unrolled NOPs, writes ceiling 0x3f, executes 0–3 NOPs and optionally
reads the threshold back, timestamps and masks MIE. The thunk's single fixed
`ret` lies between the store and K NOPs; disassembly must confirm this boundary.
The linker places it below every entry, so a saved PC before/on/after the
ceiling write can be compared numerically without confusing the call path.

A command emits 0x95 `(K, CTL, ceiling)` followed by 0x90–0x94 per trial:
variant (0–3: no readback, 4–7: readback), repeat, initial pending;
start/end/handler mcycle; entry count/interrupted PC/write PC;
first post-write PC/MIE-mask PC/readback (all ones if absent);
settled pending/total entries after release/CTL. Pending is sampled after a
4096-cycle settle with MIE off and the ceiling still high; it is outside the
timed window. Each trial then lowers the threshold and checks delivery.
One K per command keeps the existing output buffer bounded to 121 records.
There are no loops in the K or post-write instruction delays; the eight
variants each run three times. Sweep K=0–24 once to locate the admission
boundary, rather than repeating captures as a qualification series.

Reading ATTR_MODE=3 does not establish that writes are ignored: the current
read-only PAC field has no setter. Likewise, a zero LP wakeup flag before and
after reset cannot prove preservation of a pending event. These limitations
must remain explicit in a report even when the other leaves complete.

## Generic interrupt facts

| Selection | Bounded experiment |
| --- | --- |
| 40 | FROM_CPU2 low/high, MAP0 parking/unparking, IE masking, repeated level delivery, inherited PASS_LEVEL and four single-source remap settings |
| 41 | Post-ROM USB ENA0..15/RAW/ST, IN_EMPTY clear while enabled/disabled, retained pending, redelivery and all-enable masking |
| 42 / 43 | SYSTIMER target0 / target1 one-shot, WORK_EN and INT_ENA masking, clear, repeat admission, past target and a phased masked-clear/reprogram |
| 44 | TIMG0 T0 XTAL/divider-40 one-shot, counter capture, alarm self-clear, mask/clear/rearm/past target, stop and watchdog non-interference readback |
| 45 | `time`/`timeh` (0xC01/0xC81) read back to back with `mcycle`, before and after a 3,200,000-cycle window, against SYSTIMER unit 0 |
| 46 | Selection 45 with the CPU divider number one higher (applied by `SOC_CLK_UPDATE`), then the inherited divider restored |

Selections 45/46 emit 0x4501/0x4502 `(mcycle, time, timeh)` at the window's
ends, 0x4503 SYSTIMER `(first, second, delta)` and 0x4504 the three deltas;
46 adds 0x4600 (inherited root, CPU divider, numerator), 0x4601 (divider
applied, update cleared) and 0x46ff (divider restored, update cleared).
Observed on s31-a under the ROM loader (XTAL root, divider 0, 40 MHz): both
CSRs read without a trap; over the window `time` advanced exactly as
`mcycle` (3,200,133 and 3,200,135), with SYSTIMER at 1,280,036 ticks (80 ms)
and, with the divider doubled, 2,560,072 ticks (160 ms). `time` therefore
counts CPU cycles and changes rate with the CPU clock; it is not a
fixed-rate timebase. Behaviour across WFI, on hart 1 and under the PLL root
is not observed.

These explicit selections contain hart1 and use no radio or DMA. Each stimulus
or latch poll ends after 2,000,000 cycles. A missing positive cause is emitted
as an observation failure, not interpreted as interrupt suppression. The ROM
RTC watchdog is fed before each leaf and remains armed. T0 never resets its
group or writes watchdog configuration; its shared clock/enable changes keep
the other lanes and it clears T0 alone.

The generic IRQ handler receives the CLIC owner only during a bounded
admission window, disables the selected CPU line on its first entry, and
leaves the peripheral pending. Reopening that CPU line tests repeat delivery.
The thread never touches the lent owner while interrupts are enabled.

State records contain packed typed RAW/ST/ENA observations; their following
matrix record contains banks 0/1 and CLIC IP/IE/core-local IP0. A companion
record at `matrix_tag + 0x10000` contains bank2, FROM_CPU2 level and its
PASS_LEVEL readback. Probe-local packing is output encoding, not MMIO masks.
USB records also capture FIFO availability. An IN_EMPTY stimulus can require
a bounded host IN reader to consume the probe's byte; no host OUT packet is
part of the leaf. The disabled USB CLR consumes an enabled ST snapshot retained
across masking. Final RAW W1C is a distinct operation from that CLR.

The ROM loader observation is not a power-on-reset value. A completed phased
past-target example is not a guarantee for all set/clear collision orderings,
and a watchdog readback match does not qualify every active-watchdog case.

## TIMG0 reload and equal-level arbitration

Selections 190–196 use the existing TIMG0, clock, FROM_CPU2 and CLIC PAC
owners on hart0, with hart1 contained. No RF, DMA, group reset or watchdog
configuration is involved; each latch/cause wait stops after 200,000 cycles.
The existing ROM RTC watchdog is fed before each selection. XTAL/divider40
matches the HAL alarm configuration. These are isolated facts, not a
production latency or fairness qualification.

| Selection | Experiment |
| --- | --- |
| 190 | Stopped high3/low12345 seed, LOAD-register-only negative control, zero LOAD strobe, one 40,000-cycle rate interval |
| 191 | Exact #710 arm sequence from that nonzero seed for 10/100/1000 µs, once each |
| 192 | Old pending retained across 1000-µs rearm, separate CLR, fresh expiry |
| 193 | Alarm9999/10000/11000 at stopped count10000, alarm-only enable, then start |
| 194 | Running count above10000, zero-RAW control, past target0 |
| 195 / 196 | Both P2 lines pending before MIE; TIMG0/FROM_CPU2 on16/17, then17/16 |

All selections emit 0x1900 (APB gate, reset, inherited T0 source), 0x1901
(CPU source, integer divider, fractional numerator), 0x1902 (selected T0
source, divider, MNLBITS), and final 0x19ff (RAW, ST, ENA/T_EN/ALARM_EN
packed into bits0/1/2). Intermediate state records use the same encoding.
0x19ee denotes an observation/interrupt failure, independently of completion.

190 emits 0x1910/11/12 (captured low, high, zero) for seed/register-only/strobe,
then 0x1913 (first count, second count, elapsed cycles including capture).
191 emits 0x1920 (requested µs, seed low, high), 0x1921 (positive RAW,
last-negative/first-positive polling cycle marks relative to before start),
0x1922 (requested µs, arm-return delta, zero), 0x1923 (count low/high, µs),
and state0x1924. The cycle marks include MMIO and polling overhead, and are
not comparator or interrupt-entry timestamps. 192 uses 0x1930 and0x1935 for
old/fresh expiry, states0x1931/32/34/37, absolute start/return stamps0x1933,
and captured count0x1936. 193 emits target/count0x1940, states0x1941/42/44
before alarm/after stopped alarm/after start, and expiry0x1943. 194 emits
count0x1950, states0x1951/53 and expiry0x1952.

Arbitration emits expiry0x1960; assignments/CTL0x1961; state0x1962;
matrix banks0/2 and FROM_CPU2 level0x1963; line/IP+IE/CTL0x1964 for each
line; entry count/start/end0x1965; and ordered raw mcause/cycle-delta/
mintstatus0x1966 for each entry. Both IP and IE must be one before MIE.
Each handler disables only its own line through the generic IRQ module's
existing exclusive CLIC loan;
neither producer is acknowledged before the post-admission state0x1967.
The thread revokes the loan after clearing MIE. The arbitration module takes
an ordinary mutable borrow from the generic handler and forbids unsafe code.

SOURCE: `HW23_S31_*_20261027` in the register model. The shared admission
and handler path reuses the generic CLIC loan.

## Local radio interrupt retention

Of this section only selection 83 (Wi-Fi timers) is built; 80–82 and
90–105 are out of the build with their radios (see the top).

Selections 90–104 extend the BLE facts through advancing BTLC captures and
positive digital causes, including overflow acknowledgement, rate and APB-reset
boundaries. No scheduler RUN, RF operation or DMA publication is involved. The
clock/reset setup is sourced and grouped; it does not establish a minimal clock
set. With CONFIG_24=0 and CLOCK_SHIFT=3, CONTROL0=1 starts
observed capture progression. Successful seed/capture correspondence alone
is insufficient; subsequent leaves require a nonzero capture delta.

| Selection | Bounded experiment |
| --- | --- |
| 90 | Completed/stationary seed capture, sourced mode change, CONTROL0=1, then separate LP start; paired captures at each boundary |
| 91 | CC0/CC1 crossings and seeded first-word wrap; primary/NRT banks and matrix124/133 |
| 92 | Each positive cause across mask/unmask, retained-snapshot masked clear and arrival while masked |
| 93 | Already-pending overflow across CONTROL1, CONTROL2, their pair and CONTROL0 reopen |
| 94 | Each baseline/dynamic mask and the production release suffix with overflow positively pending and ENA1[1] retained |
| 95 | CC0 snapshot followed by CC1 arrival and sampled ack; separated same-bit target/ack phases |
| 96 | Two positive LP lanes across +0x34 all-ones, then masked-zero observations after that destructive clear |
| 97 | Sleep16 target crossings, +0x1c, independent mode58 controls0/4 and matrix127/129 |
| 98 | Sourced sleep0/17 command pairs, ETM companions and +0x68 target; no invented read of +0x30 or +0x68 |
| 99 / 100 | #685 LP stop order, controller reset, no-MMIO reunion boundary, digital clock release/restoration with isolated software / one real rollover and primary overflow |
| 101 | Positive masked-zero checks for both LP lanes before +0x34 clears anything |
| 102 | BTLC overflow with baseline ENA1 0x1302: mask/unmask, NRT sampled-copy clear, primary sampled bit-1 W1C and short quiet window |
| 103 | One BTLC rate interval bracketed by mcycle, clock/divider readback and one near-wrap seeded crossing |
| 104 | Positive overflow and every ENA0/ENA1 lane before MAC reset, after APB reset, after baseline re-enable and CONTROL0 reopen; fresh wrap control |

Each BTLC latch/cause poll is bounded by 200,000 cycles. Each LP wrap wait
is bounded by 40,000,000 cycles and requests one real wrap; ordinary windows
use 4096 cycles. Primary records hold the two raw status images and matrix
bank3, then NRT images and matrix bank4. Following records encode selected
enables explicitly; they are not raw enable words. Task enable17 is sampled
separately. Selection 104's ENA0 records report `iso_cig_reset_source` in
the third value at enable-record offset 2 and `bits_3_7_opaque` in the
third value at offset 4; every other field retains its position.
LP records hold counter/status/matrix3, bytes24/2c/matrix4,
bytes1c/20/task17, and mode58/target64/target6c. No gated peripheral read
qualifies output-low; the matrix remains the clock-off observation.

The measured CC0/CC1/wrap causes raise124 while their NRT snapshots stay
zero, so those snapshots are not a RAW alias for these primary causes.
CONTROL1/2 and the release masks do not silence already-pending inherited
source1. Sleep16 has a positive flag but no qualified IRQ route: its delivery
bank was disabled. Unknown sleep conditions, bank0 producers and a positive
133 producer remain unqualified. SOURCE: `HW9_S31_*` in the register model.

| Selection | Bounded experiment |
| --- | --- |
| 80 | LP compare arrival after a zero read and before zero acknowledgement; clear-before-arrival control; retained nonzero sample across a rearmed target crossing |
| 81 | Exactly two natural wraps with compare disabled and no intermediate ack; one final zero and a 4096-cycle observation |
| 82 | IEEE timer0/timer1 expiries while enabled/masked; hidden pending revealed by unmask; zero masked snapshot versus a retained positive snapshot acknowledged while masked; selective enabled W1C |
| 83 | Local TSF timer0/1 RAW, STATUS and source122 across mask/unmask; retained sampled acknowledgement while masked, enabled acknowledgement, and production mask/all-clear |

These explicit leaves use the session's existing PAC claim. CPU interrupts
stay masked, hart 1 stays held, and sources 122/132 are parked on hart 0;
only matrix status is observed. There is no RF operation, RX/TX command,
DMA publication, scheduler or protocol. ED, TBTT/beacon and packet causes
are not exercised. Digital clock/reset setup follows the production module
prerequisites; it does not establish their minimal necessary subset. Wi-Fi
uses the slow-oscillator LP clock with divider zero and opens the bootstrap
SYSTIMER clocks. The LP leaves retain exclusive XTAL/divider one.

Each LP wait is bounded by 40,000,000 probe cycles; selection 81 requests
exactly two wraps. Each IEEE/TSF cause poll is bounded by 200,000 cycles;
ordinary observations use 4096 cycles. Expiry with no positive cause is
inconclusive. Selection 82 deliberately samples masked status: a zero
snapshot need not acknowledge hidden pending. It then uses a positive
snapshot retained across masking as an independent clear control.

In selection 82, records hold compact timer0/timer1 status flags 1/2,
compact enables 1/2 and the full matrix bank4; the next record holds timer
values. Selection 83 records compact RAW flags 1/2, actual STATUS bank bits
and full matrix bank3; its next record holds station TSF low/high and
compact enables. The compact flags are result encoding, not MMIO masks.
Selection 80's retained-one byte cannot independently count a second
compare. None of these phased experiments establishes simultaneous
set/clear priority, arbitrary wrap capacity or an exhaustive all-cause stop.

Selections 105–108 establish, respectively, one real compare, one natural
rollover, both, and neither, with both LP delivery lanes enabled. Each checks
the exact pending-byte precondition, then on hart0 with hart1 already held:
threshold ceiling, MIE off, held confirmation, LP snapshot, UART0 XTAL
restoration, boot-address zero, other-core reset pulse, stall, retained SRAM
record publication, fence, SYS_SW_RST, WFI. There is no timer cleanup before
this reset.

The successful path preserves the reset-control register-effect order of
production `fatal::reset_recording`: UART0 restoration, boot-address zero,
other-core reset pulse, stall, fence and SYS_SW_RST. The complete sequences
differ: the probe publishes its retained SRAM record after the
UART0/boot-address/pulse/stall writes, whereas production's record callback
runs before them. The probe has no ClaimCell election or StopError handling,
and does not implement production's watchdog-fallback path. A failed hold
check returns from the leaf with MIE cleared. The comparison covers those
successful reset-control effects and the next ROM-loader entry, not full
production fatal-path equivalence.

On the next ROM MEM_END entry, the retained LP reason automatically selects
109. No debugger reset or re-entry of `_start` may intervene. The observer
first samples the complete matrix status word containing source 127, LP
cause, pending bytes and counter (0xb100/1), plus inherited clock fields
(0xb102/3), before any peripheral write. It emits the retained pre-reset
record at 0xb110–13 and HP reset causes/record validity at 0xb114. A separate
clock-only restoration phase samples at 0xb120–23 without preparing the
LP timer or writing +0x10. A separate LP XTAL/divider restoration, still
without clear/prepare/start/enable, samples at 0xb124–27 to distinguish a
source-clock-hidden state. These phases must not be conflated if clocks
make the initial bus image unqualified.

`+0x10` has no modeled read accessor. A fresh software cause tests retained
bit18 delivery before/after an explicit enable (0xb130/2). With only compare
disabled, a single natural rollover after counter-clock/control25/start restoration
tests bit1 delivery before/after enable (0xb142/4). If the disabled wrap
produces no pending byte, a fresh enabled wrap is the positive control
(0xb148); this distinguishes a masked producer from a broken stimulus.
No all-ones disable precedes that test. Waits are bounded to 40 million cycles for one rollover,
200 thousand for compare, and 4096 for a diagnostic settle. A missing
precondition, timeout or absent positive control invalidates the affected
claim; completion alone is not success. The reason is consumed once.

## Opaque digital observations

Selections 10–14 are independent commands, excluded from selection 0:

| Selection | Experiment |
| --- | --- |
| 10 | P0: six-field PLL-gate readback, old `3d→25` and staged `15→1d→2d→25`, once each |
| 11 | P-BA: ordinary hardware index 0 and extra index 3, quiet staging and latched readback |
| 12 | BB-L: digital BB reset/gates/interface prefix, reset action disabled, `c0010001` candidate, one 4096-cycle observation and IRQ/clear toggles |
| 13 | P-TSENS: four bit9/bit23 cases, two sample edges each, up to 99 reads at 10–990 µs under a 1 ms deadline per edge |
| 14 | P-I2C: DATA-seeded serial/parallel/serial reads, real TXRF map-clear edge, mask/host negative controls and restored positive reads |

P0 and TSENS refuse timing attribution unless the owned CPU clock words show
undivided XTAL with fractional numerator zero. On refusal, the leaf emits
record tag `0xaf` with `(a,b,c)=(10,0,0)` for P0 or `(13,0,0)` for TSENS;
`a` is the decimal selection ID. The preceding `0xa0` record contains the
observed CPU root/divider fields. The experiment returns before its hardware
writes. The normal completion word may still be `0xfac70001`: `0xaf` means
the experiment was skipped, not that the hardware claim passed.

P0 touches only HP/LP words
while changing the modem source; a BTLC latch is not safe evidence before its
clock path is independently established. It observes six named bits, preserving
reserved lanes, and establishes neither PLL lock nor transition glitch freedom.

The BA/BB prefix selects the XTAL modem source and active digital clock maps,
keeps MAC disable code 15, and enables the digital BB clocks/interface. It does
not initialize RF, publish DMA or start a radio scheduler. The session is
disposable: return it to ROM download with the leased stand reset after capture.
These are not production frequency or calibrated-PHY conditions.

### P-I2C transport prefix and controls

Selection 14 uses a **probe-specific transport prefix**, not production
`power.rs::execute_owned` or complete `phy_rf_init`. It accesses only the
existing PAC owners. The following differences delimit the result; source
paths below are under `chips/esp32s31/`.

| Production operation / order | Selection 14 difference and reason |
| --- | --- |
| `hal/src/power/chip_clocks.rs` adopts the Wi-Fi power gate before radio power. | The leaf opens that gate directly at entry: it has the sole PAC owner and no platform clock manager. |
| `hal/src/power.rs::execute_owned` acquires an MPLL reference first; the PSRAM mapping already holds its permanent reference. | No refcount acquisition or PLL-power action: this SRAM facts image has no PSRAM mapping, HAL manager or assumed calibrated/permanent MPLL/BBPLL epoch. Gate readback and completed commands are the observations, not PLL readiness. |
| PMU ICG selection/updates → register bus → syscon/shared maps → REF160/source → BB reset → calibration gates → I2C160/master gate. | These write edges keep that order. All six syscon maps use the production OR updates, including ZB4 and FE6; reserved/unrelated lanes are preserved. The leaf records selected fields, not all production `PowerClockReadback` checkpoints or their typed admission failures. |
| `phy/src/analog/i2c.rs::PhyRfInitPrefixTransition` begins with `FeBbClock`: FE `0400`, FE `0800`, BB `0c80`, then PMU `enable_frontend_baseband_power`. | All four operations are omitted. The question is the APB analog master, not functioning FE/BB engines; the independently enabled modem/APB master is tested directly. No FE/BB readiness or minimal-gate necessity is claimed. |
| BBPLL calibration-mode MMIO, then bias analog setup precede `OpenI2cXpd`. | Both are omitted to leave calibration state unmodified and avoid analog writes. A successful read does not establish calibrated PHY state. |
| `OpenI2cXpd`: RF power off, BB-I2C tie release, 100 µs, RF power/tie on, conditional peripheral-I2C power/reset, then early SDM read/deadline loop. | The finite power/reset edges and 100 µs delay are retained. The false BB-I2C tie emits no write as in current HAL. The SDM read, stability loop and MAC-counter deadline are omitted: they admit a calibrated/stable SDM epoch, outside this transport fact. |
| SDM stability → 10 µs post-delay → PBus clear → six timing writes → analog BBPLL enable → analog/MMIO ADC rate → register mode/enable. | The post-delay, PBus clear, analog BBPLL enable and ADC-rate operations are omitted; they prepare calibration/signal paths and include analog writes. All six timing writes and register mode/enable run **before the first read**, intentionally testing this initialized transport rather than production's default-timing first SDM read. |
| Remaining RF-init suffix: PWDET, FE, TSENS/background, RC/filter/DCAP, analog init, RFPLL charge pump, command memory, SAR2, crystal-duty and channel/frequency work. | Entire suffix omitted: no calibration, tone, channel switch, initialized frequency memory or traffic is required/claimed by this leaf. |
| Production owns timer edges, critical-section/lifecycle state, shared clock references and failure handling. | The disposable facts session has masked interrupts, hart1 held and no executor. A verified 40 MHz CPU XTAL supplies the 4000-cycle pre-delay and 40000-cycle deadline per host. No shared clock refcounts are manufactured; cleanup returns the session to ROM download. |
| Normal read command DATA=0, normal routing; ROM parallel set-first/clear-second publication. | Every command uses a stated nonzero DATA seed with READ_WRITE=false; this separates returned zero from unchanged DATA. Before parallel reads, TXRF_MST0_SELECT is set to1 without a command, then BBTOP is set and TXRF is cleared through typed fields, with all three readbacks recorded. This verifies a real `1→0` storage edge. |
| Production uses its admitted route only. | Two bounded negative controls deliberately disable all read selectors, then route BBTOP to host0 while issuing its read on host1. Each is followed by a restored positive BBTOP read. A changed result or timeout is expected relative to positive BBTOP; no exact no-responder byte is assumed. No analog writes occur. |

The positive sequence reads BBTOP04 and TXRF08 serially on host1, then in
parallel (BBTOP host0, TXRF host1), then serially on host1 again. BBTOP seeds
are `a5/5a/3c`; TXRF seeds `a5/c3/5a`. The mask-negative and host-negative
BBTOP reads use `a5`, restored positives `5a` and `3c`. A timeout or busy
refusal stops further commands without retry; after a negative timeout the
remaining control/restoration has no result. Every actual result must be
reported; final completion does not assert an expected data value.

All tags below are hexadecimal. Completion word `fac70001` only means the
leaf returned. `16f` means the six positive reads completed; `1ff` means
both negative controls and their restored positives also completed. Neither
marker means the routing hypothesis passed.

| Tag | Record `(a,b,c)` |
| --- | --- |
| `a0` | CPU root, integer divider, fractional numerator/denominator; shared with P0/TSENS. |
| `140` | `(0,0,0)`, prefix entered. |
| `141` | peripheral-I2C power, peripheral-I2C reset released, master clock gate. |
| `142` | I2C160 source select, REF160 output gate, PHY register enable. |
| `143`, `144`, `145` | BBTOP_MST0_SELECT, TXRF_MST0_SELECT, upper-map diagnostic image; after preset TXRF1, set BBTOP1, clear TXRF0 respectively. |
| `14b` | `(case base, requested block, requested byte)`, no modeled route; leaf stops. |
| `14c` | `(14,0,0)`, CPU clock refusal before the prefix; preceding `a0` gives observations. |
| `14d` | `(case base, host,0)`, selected host busy before command publication; route/mask may already be set. Leaf stops. |
| `14e` | `(170,busy0,busy1)`, parallel admission refused before lane changes; leaf stops. |
| case base `+0` | reconstructed ANA_CONF1, BBTOP_MST0_SELECT, TXRF_MST0_SELECT. |
| case base `+1` | returned data, number of busy polls, completion-observer cycles. **Data=`ffffffff` means timeout**, not an analog byte; `+2` is absent and the leaf stops. |
| case base `+2` | command block, command byte, final busy. |
| case base `+3` | upper-map diagnostic image, register mode, register enable. |
| case base `+4` | host, DATA seed,0. |
| `16f` | `(6,0,0)`, six positive commands completed. |
| `1ff` | `(1,0,0)`, entire leaf completed. |

The upper-map diagnostic image packs the named lanes above TXRF and the two
opaque gaps using generated field positions, with the SDM lane as bit zero.
It preserves the record's twelve-bit encoding without an aggregate register
field. SDM, RFPLL, BIAS, BBPLL, ULP_BOD, SARADC and DIG_REG lane names describe
the ROM publication combined with the modeled normal-profile read selectors;
their independent silicon effects are untested. The unmatched gap positions
stay opaque. HW15 is evidence for the observed bytes, BBTOP controls and TXRF
stored clear; the TXRF route itself is sourced only from the vendor mapping.

Case bases: `150` initial BBTOP, `160` initial TXRF, `170` parallel BBTOP,
`180` parallel TXRF, `190` final BBTOP, `1a0` final TXRF, `1b0` no-selector
BBTOP, `1c0` restored BBTOP, `1d0` wrong-host BBTOP, `1e0` restored BBTOP.
Each base is emitted once; refusal tags are disjoint. `14f` is unused.
Parallel host1 progresses while host0 is being observed; observer cycles
start after publication and do not measure a complete command duration.
Use a held stand lease and return the disposable session to ROM download.

Record tags: `a0` CPU clock fields; `a1` initial MODEM_CONF/PMU code; `a4`,
`a8`, `ac`, `b0`, `b4`, `b8` P0 step readback/PMU code/cycle. For BA, `b0–b1`
are clock maps/gates/reset observations. Pairs `c0–cf` contain CONTROL,
peer/policy/head, sequence and bitmap: initial ordinary/extra; ordinary first,
second and post-update; extra first, second and latched. Full modeled words
are reconstructed from typed getters and generated field geometry.

BB groups `d0,d3,d6,d9,dc,df,e2` contain status/control/enables,
MAC RAW/ST/matrix bank3, then MAC disable/ordinary peer marker/zero. `+4c`
has unknown access semantics and no PAC accessor; the current leaf never
reads it and emits no group-plus-`100` post-read observation, so it does not
prove a side-effect-free read or read-to-ack.
`e5` records the candidate's observed status. All tags are hexadecimal. A zero
producer result does not authorize a reset-scope claim, read-to-ack semantics,
clear effectiveness or a universal source123 silence claim.

TSENS schedules at most 99 in-window reads per edge at offsets 10–990 µs.
The nominal 100th iteration waits until 1000 µs and is rejected by the
`>= 1 ms` deadline before reading; an earlier overrun may reduce the count.
The before-edge CTRL read is separate. The output does not record an exact
sample denominator; ready-sample and OUT-change counts are bounded by 99.

TSENS pairs `f0–ff` report before READY/OUT, ready-sample count, final READY/OUT
and OUT-change count, requested images 11,01,10,00, two edges per case. Tags
`110–113` verify CLK_CONF (semantic bits: bit9, bit23, CLK_EN), divider,
and power/clock (POWER_UP, POWER_UP_FORCE, LP clock, LP reset). Named LP clock,
forced power and divider15 are fixed; the saved power/clock/divider/sample
fields are restored afterwards. Changing OUT with READY always zero does not
identify individual conversion/readout gates or establish a READY handshake.

## SAR/PWDET lifetime and freshness

Selections 140–146 and 149 use the same one-time PAC claim. Run 140 once
before the others. It applies a finite subset of the production modem clock,
FE/BB and PWDET initialization leaves in the order detailed below, selects the existing reference
calibration mode, and checks references 0 and `0x5555`. It performs no RF/PHY
calibration, PBus gain/DCO setup or background tracking. The measured epoch
starts from ROM-download state, not an untouched power-on reset.

These selections execute in an isolated P2 software-interrupt context using
the existing trap ABI; `0x14f0` records `mintstatus` and `mcause`. Cycle-counter
boundaries advance the observations at the adopted CPU rate. Only an integer
MHz XTAL/CPLL-derived rate is admitted. Named conversion observations stop
at the 100 µs experimental boundary, which is not a production timeout.
There is no production executor or timer-service latency in this image.

| Selection | Operation |
| --- | --- |
| 140 | Initialize, explicitly disable background PWDET, capture two reference conversions; refuse later trials without nonzero samples |
| 141 / 142 | Repeated reads at nominal ready+0/1/10/100 µs / first read deferred to ready+100 µs |
| 143 / 144 | Same two HOLD trials, clearing tone arm immediately after observing ready |
| 145 / 146 | Alternate reference 0/`0x5555`, with early result reads / first post-trigger result read deferred to 100 µs; require distinguishable preflight samples |
| 149 | Stop both tone paths, restore DAC scale, release TSENS and clear the calibration clock; the host then uses the stand's download reset |

The first result word's upper lane is the measured sample. Arm settling is
at least 1 µs; ordinary ready polling begins after at least 2 µs and accepts
only the named three-bit value 7. EDGE additionally observes status early,
without accepting early data as completed. RAM records are published after
the timed region; the debugger reads only the SRAM output and publishes
selectors while halted. No peripheral MMIO is performed by the debugger.

Record tags (hexadecimal): `1400/1401` clock selection/rate; `1406/1407`
background-enable and mode checkpoints; `1402/1410` cycle stamps surrounding
the trigger-setting PAC RMW, then `1403/1411` elapsed cycles/status per poll;
`1404/1405` preflight samples/admission; `1412` delayed-first-read/arm-clear
flags and ready anchor; `1413` nominal delay/start/end cycles from that anchor,
followed by `1414` ready-before/sample/ready-after; `1450` reference and sample
before/after publication; `1454` ready before/after reference and after arm;
`1451` trigger stamps/deferred-read flag; `1452` nominal delay/start/end cycles
from the post-trigger stamp, followed by `1453` ready/sample (`u32::MAX` when
intentionally unread); `1490` cleanup observation. `14ee` is a refused or
failed experiment, even if the ordinary completion marker is present.

The ready anchor is the cycle read immediately before the first status load
that returned 7. Nominal deadlines are lower bounds: the recorded start/end
cycles expose late observations and MMIO overhead. The actual trigger store
lies inside the two trigger stamps. Do not infer exact nominal observation
times, physical minimum conversion time, indefinite retention or calibrated
voltage/power from these records. SOURCE: `oer-notes/research/opaque-blobray-phy.md`
(P-SAR-HOLD/P-SAR-EDGE); silicon results: `oer-notes/research/hw-sar-hold.md`.

### Preparation scope and remaining differences

The finite production prefix follows `hal/src/power.rs::execute_owned`
and `phy/src/analog/i2c.rs::PhyRfInitPrefixTransition` through TSENS:
common modem power/readbacks; whole-word PLL images `0x15`, `0x1d` (XTAL
disabled); calibration clock; FE/BB clocks; BBPLL mode; bias writes;
`open_i2c_xpd` RF power-down, 100 µs wait, power-up and conditional SAR-I2C
reset assert/release; SDM byte `0x5b`; 10 µs post-delay; twelve PBus-clear
transactions and conditional work-mode pulse; I2C timing, BBPLL clear/read,
high ADC rate, master mode; PWDET tables; seventeen FE writes; TSENS cold
readout and first-acquire reset/power sequence. Every MMIO uses existing
PAC owners. Every analog byte resolves both host and command alias through
`LibphyNormal`, including writes. A missing SDM/readback or a timed-out
byte/PBus command refuses the experiment; no fallback preparation is used.

The following differences are material; these observations do **not**
establish a production lifetime contract on their own. The same ledger is
SOURCE `HW17_S31_SAR_PREPARATION_20261027` in the model.

| ID | Difference from production | Reason / consequence |
|---|---|---|
| D1 | ROM RAM-loader state, SRAM stacks and adopted integer-MHz CPU rate; no staged bootstrap/PSRAM, MPLL reference, platform reference counts or Wi-Fi client cold baseline/LP-clock selection/combined BB+MAC reset entry | No Wi-Fi client epoch is opened; a standalone MAC pulse would not reproduce that parent lifecycle. PAC-only image has exclusive words, not the production platform lifecycle. Parent PLL/bootstrap state is inherited and recorded where observable. No production clock/load equivalence is claimed. |
| D2 | SDM checks the same `0x5b` predicate with a 100 µs `mcycle` deadline instead of 9999 MAC-local counter ticks; byte/PBus operations have 20 µs experimental bounds; SAR has a 100 µs bound | Required short stand scope. The ordinary production timer/40 µs policy and vendor unbounded wait are not copied. Named settles preserve ≥100/10/1/2 µs; no executor timing equivalence. |
| D3 | The production background-enable bit after TSENS is deliberately kept 0 before manual reference/tone selection | Prevent autonomous triggers during HOLD/EDGE; retention with background tracking remains untested. |
| D4 | RC settings/calibration, filter DCAP and parameter reads, analog init1, RFPLL charge-pump search, command RAM publication, later conditional SAR2 initial-code write, FE update, crystal-duty and channel initialization are omitted | They belong to the full RF calibration suffix and depend on retained/calibrated parameters. No synthetic substitutes are used. In particular `0x578` is not moved before the test conversions. |
| D5 | Full BB/RX/TX DC/IQ calibration and the reference-code root's 15 calibrated PBus setup operations / forced TX clock are omitted | No calibrated RF epoch is available without the prohibited full bring-up. Only the finite existing internal reference/tone leaf is exercised; electrical quality and production transfer remain unproven. |
| D6 | Sole-owner tone stop / DAC restore / TSENS release and calibration-clock clear, then host stand download reset; no shared route restore or background-tracking handback | There are no other PHY clients in the image. Full-cleanup retention is not measured. |
| D7 | Isolated P2 handler with cycle boundaries, not the production P2 executor/timer service; some nominal +0/+1 points are late | Actual read windows are logged. No completion IRQ, sub-µs timing, continuous ready retention or loaded-executor behavior is inferred. |

### Complete SAR record arguments

All records are four `u32` words `(tag, a, b, c)`. Tags below are
hexadecimal; booleans are 0/1, unused arguments are explicitly zero.
Absolute cycle stamps wrap at 32 bits. Elapsed cycles use wrapping
subtraction at the adopted CPU frequency; divide by `hz / 1_000_000` for
microseconds. The actual trigger-setting store is between its before/after
stamps. Poll elapsed times start at the after stamp. The HOLD ready anchor
is the stamp immediately before the first status load returning 7; it is not
a timestamp of the hardware ready edge.

| Tag | `a` | `b` | `c` |
|---|---|---|---|
| `1400` | `SOC_CLK_SEL` root code | CPU integer divider code | CPLL feedback divider |
| `1401` | Adopted CPU frequency `hz` | CPLL reference divider | CPU fractional numerator |
| `1402` / `1410` | Absolute cycle stamp before the trigger-setting PAC RMW | Absolute cycle stamp after that RMW | Adopted CPU frequency `hz` |
| `1403` / `1411` | Elapsed cycles from the after-trigger stamp to completion of this status read | Raw three-bit `SAR_READY` code | 0 |
| `1404` | Preflight reference (0 or `0x5555`) | Upper-lane sample if ready was accepted; otherwise 0 without a result read | Whether `begin()` accepted ready=7 (not whether the sample is nonzero) |
| `1405` | `usable`: both preflight samples are nonzero | `distinct`: usable and the two samples differ; this is only an inequality gate | Current three-bit `SAR_READY` code |
| `1406` | Saved `POWER_DETECTOR_CONTROL.BIT_16_OPAQUE` background-enable bit | Current three-bit `SAR_READY` code | Saved `POWER_DETECTOR_CONTROL.BITS_20_22_OPAQUE` field |
| `1407` | Current background-enable bit | Current `POWER_DETECTOR_SAR_CONTROL_STATUS.BITS_12_13_OPAQUE` field | Current `RTC_SAR2_PWDET_CCT` field |
| `1408` | Common-power checkpoint number 1–8 | Observed boolean | Expected boolean (1) |
| `1409`, stage 1 | 1 (SDM predicate accepted) | Accepted SDM byte (`0x5b`) | Elapsed cycles since the SDM polling loop began after power/reset completion |
| `1409`, stage 2 | 2 (cold prefix through I2C master setup completed) | 1 (success) | 0 (not a duration) |
| `1412` | Delayed-first-result-read flag | Tone-arm-clear-after-ready flag | Absolute HOLD ready-anchor cycle stamp |
| `1413` | Nominal delay from the ready anchor, in µs (0/1/10/100) | Start elapsed cycles from that anchor | End elapsed cycles from that anchor, after ready/result/ready reads |
| `1414` | Ready immediately before the result read | Upper-lane sample | Ready immediately after the result read |
| `1450` | Published reference (0 or `0x5555`) | Sample immediately before reference publication | Sample immediately after publication, without a new trigger |
| `1451` | Absolute cycle stamp before the trigger-setting PAC RMW | Absolute cycle stamp after that RMW | Defer first post-trigger result read until nominal +100 µs flag |
| `1452` | Nominal delay from the after-trigger stamp, in µs (0/1/2/3/5/10/20/100) | Start elapsed cycles from that stamp | End elapsed cycles after status and, when enabled, result reads |
| `1453` | Three-bit ready code at this EDGE observation | Upper-lane sample, or `u32::MAX` when deliberately unread | 0 |
| `1454` | Ready before reference publication | Ready after reference publication | Ready after arm and its ≥1 µs settle, before triggering |
| `1490`, powered | Current ready after both tone paths stop and DAC scale is restored | Current upper-lane sample at that point | 0 |
| `1490`, unpowered | `u32::MAX` (unavailable) | `u32::MAX` (unavailable) | 0; this branch performs no cleanup MMIO |
| `14ee` | Refusal/failure reason | Reason-specific detail (table below) | Reason-specific observation (table below) |
| `14f0` | Selected SAR probe ID | Raw `mintstatus` at handler dispatch | Raw `mcause` at handler dispatch |

`1406` is recorded after the common-power/cold prefix and before explicit
background isolation and PWDET table initialization. `1407` is recorded at
the end of configuration, after selecting calibration circuit mode and the
tone image. The `1490` powered observation precedes TSENS/SAR-I2C release and
calibration-clock clear; it does not measure retention after full cleanup.
The `1408` checkpoints are, in order: reset release, active ICG, register
bus, modem maps, shared maps, PLL source/XTAL-off, PHY clocks and I2C source.
Each `1413` is followed by its `1414`; each `1452` by its `1453`.

Common image diagnostics applicable to SAR commands are also four-word
records: `ed` for an unknown selection is `(requested ID, 0, 0)`; `ee` for
excess unexpected IRQ entries is `(raw mcause, prior IRQ count, absolute
handler-entry cycle stamp)`; `ef` for an exception is `(raw mcause, mepc,
mtval)`. These are failures, not SAR samples.

`14ee` is `(reason, detail, observation)` and always means failure/refusal,
even with an ordinary completion marker:

| Reason | Meaning | Detail / observation |
|---:|---|---|
| 1 | Unsupported/fractional/invalid adopted clock | 0 / 0; consult `1400/1401` |
| 2 | Analog field's read failed | block / byte index |
| 3 | Value does not fit the generated analog field | old byte / requested value |
| 4 | Analog initialization failed | stage 1=bias, 2=BBPLL, 3=ADC-rate / 0 |
| 5 | HOLD refused: no admitted nonzero preflight | 0 / 0 |
| 6 | EDGE refused: no distinguishable nonzero references | 0 / 0 |
| 7 | Invalid SAR registry selection | requested selection / 0 |
| 8 | P2 handler did not complete | requested selection / 0 |
| 9 | Common-power readback mismatch | checkpoint / observed boolean |
| a | SDM did not return `0x5b` within the short bound, or transport failed | last byte (`u32::MAX` if unavailable) / elapsed cycles |
| b | PBus-clear command was busy at start or timed out | selector / path |

### Observed limits

The corrected-prefix capture contains ten conversions (two preflight, four
HOLD, four EDGE) on one board. HOLD retained data/ready through the observed
100 µs window. Partial ready codes were nonmonotone. Preflight reference
samples 1969/1963 were unequal, so the gate admitted EDGE, but EDGE final
reference outputs differed by only 1–2 codes, comparable with nearby
conversion variation. This does not establish a robust reference-specific
stimulus or causally attribute the final result difference.

## Generic interrupt follow-up

Selections 220–229 use the same PAC claim and existing clock setup. They are
short local register experiments, with RF gates closed, no DMA publication,
no radio traffic and hart1 contained. They do not qualify a production image.

| Selection | Question |
| --- | --- |
| 220 / 221 | Load/snapshot generic counter2 / counter3 and arm its timer: positive RAW, mask, retained-sample CLR and unmask |
| 222 | Station TBTT from a local loaded target: the same mask/CLR sequence |
| 223 / 224 (out of the build) | Digital ED completion / ED followed by STOP: event visibility and clear with enable zero/one |
| 225 | First post-loader USB RAW/ST/ENA observation; enable OUT/IN and clear the sampled console events |
| 226 | One host OUT byte after 225, retained-sample masked CLR with FIFO still occupied; two consumed IN publications; explicitly seeded SOF enable preservation |
| 227 | Fresh target0 expiry after CLR, acknowledgement after expiry, then CLR/rearm and another fresh expiry |
| 228 | Targets0/1 at the same programmed tick; target0 CLR must preserve target1 |
| 229 | Unit0 LOAD near the low-word and full counter boundaries; target0 across each boundary with CLR before counting |

Run 225 before 226 in one loader session. Send exactly one host OUT byte
between those commands while halted; keep a host IN reader open for 226.
The inherited SOF case is explicitly constructed, not an observation of a
boot image. The endpoint enable RMW and console-only acknowledgement reproduce
the production register effects without a HAL dependency.

Radio waits are bounded by 200,000 CPU cycles. SYSTIMER/USB waits are bounded
by 2,000,000 cycles each, and ordinary observations by 4096 cycles. ED energy
values without RF calibration are not measurements. A completed leaf without
a positive cause establishes no mask/clear relation for that cause.
Wi-Fi wave2 RAW records compact timer2/timer3/TBTT0 as bits0/1/2; STATUS and
matrix words retain their physical bits. IEEE records compact ED_DONE/RX_ABORT
as bits0/1, with enables encoded identically; the following record contains
RX abort reason and MAC state. Generic USB/SYSTIMER records retain the existing
encodings. Selection229 records the seeded and later full counter values and
a separate observed-boundary flag; an early RAW is not a fresh post-wrap expiry.

The leaves observe positive local timer2/3 with their own counter snapshots
and local TBTT0; all three retain RAW under mask and clear from a retained
positive sample while masked. They separately establish RX_ABORT reason26
(ED_COEX_REJECT), positive USB OUT and fresh IN events, and the three ordered
SYSTIMER cases. ED_DONE/ED_STOP, other event lanes and exact coincident
set/clear priority remain unqualified.
SOURCE: `oer-notes/research/hw-irq-generic-2.md`.


## Local TBTT and interrupt observers

Selections 260–273 are explicit short PAC-only leaves with RF/DMA closed,
CPU admission masked and hart1 contained. Each new leaf first validates both
CLIC positive-edge observers with FROM_CPU2 while IE remains zero. Radio
routes122/132 stay parked;123/131 use separate validated edge inputs with
CPU delivery disabled. A quiet radio input never qualifies a producer.

| Selection | Experiment |
| --- | --- |
| 260 /261 | Timer2/3 against its own advancing, loaded TSF: +1024 target, stop, mask/unmask, retained-positive masked CLR, enabled CLR, masked expiry, stopped crossing and policy/rearm |
| 262–265 | The same boundaries for TBTT0–3: interval8, target selected snapshot+8TU, ahead0 and its shared load selector |
| 266 /270 | Two positive timers / TBTT pairs0+1 and2+3; selected CLR preserves the other cause |
| 267 | Seeded BBTOP04/TXRF08 serial reads, parallel host routing, then serial recovery; BUSY/DATA, host status, burst status, matrix123/131 and edge IP |
| 268 | Normal read, a bit26-only publication conditional on a fresh BUSY read, then a normal recovery read; matrix131 recorded separately |
| 269 (out of the build) | IEEE timer0 threshold32 with route132 parked; positive masked pending revealed on unmask and cleared from its saved sample |
| 271 /273 | TBTT0/1 /2/3 restart after a stopped target crossing: sourced run-before-schedule order, explicit stale discard, future crossing, then policy with run left active |
| 272 | Timer2/3 run readback before and after expiry/policy; explicit rearm and fresh masked expiry |

Timer/TBTT waits and the stopped-crossing interval are bounded by 800,000 CPU
cycles (20 ms at the checked 40 MHz XTAL); I2C commands by 40,000 cycles (1 ms).
Operation stamps include observer overhead and are not bus-completion timestamps.
Radio state tags carry `(RAW low8, STATUS, matrix bank3)` and tag+1 carries
`(ENABLE low8, timer run bits0–3, TBTT run bits0–3)`. These are physical low8
positions; upper power-bank lanes are not qualified. Tag31ee is a failed
positive/crossing precondition, independently of the completion marker.

Observer tags3001 encode `(input index, beforeIP | afterIP<<1 | IE<<2,
matrix bank2)`. Command sample triples carry `(cycles, BUSY0|BUSY1<<1,
DATA0|DATA1<<8)`, `(matrix bank3, bank4, edgeIP0|edgeIP1<<1)`, then
`(host status0, status1, burst DONE|ERR0<<1|ERR1<<2)`. Four triples bracket
publication/conditional reset/first idle; the following records aggregate
observed BUSY, actual reset publications, polls, matrix inputs and final IP.
Tag34f0 records matrix bank4 before/after the conditional bit26 path and the
actual publication mask. A skipped reset is not a reset observation.

SOURCE: `oer-notes/research/hw30-tbtt-irq-observer.md`; captures retain their
exact loaded ELF and source patch. The legacy selection 83 is used only as a
PWRBANK timer0/1 harness control; ED6 and radio traffic are outside these leaves.

## TSENS and analog/PMU facts (PB-1e)

Observation waits use the initialized SAR owner's ticks-per-microsecond conversion.
HW31 uses selections280–299; HW29 retains240–248 and HW30 retains260–273.

Selections 280–283 require a separate power-on/download loader entry for each
arm: no opaque-one publication, bit23 alone, bit9 alone and the ordered
bit23/bit9 pair. They use the existing probe-specific analog-I2C prefix, named
SAR-I2C and TSENS reset, CLK_EN, divider15, forced power and SAMPLE_EN. This
is not a calibrated PHY or FSM-power epoch. DAC15 settles for 500 µs before
DAC11 and DAC15 observation windows of 500 µs each. Each step records actual
CPU-cycle timestamps at requested 0/1/2/5/10/20-µs boundaries and every 10 µs
thereafter; observer overhead can make the early observations late. The
reference is completion of the typed DAC write, not its unknown analog edge.
A separate DAC-byte readback follows each window. OUT noise or READY alone
cannot replace the ordered-pair arm's DAC-correlated positive control.

Selections 285–299 are cold omission/injection arms. Groups of five select
bit23, bit9 and the ordered pair; the five patterns are no injection, one,
one followed immediately by zero, repeated ones at +50 µs and at +500 µs.
The first injection follows the 20-µs sample of the DAC11 step, whose window
extends through 530 µs. The DAC15 return step has no new injection. Each
command emits at most 127 of the existing 128 four-word records. Clock,
power and reset state precede/follow each command; CTRL and CLK_CONF are
sampled together in each observation. `0x31ee` reports an unqualified setup,
DAC or disabled-CONTROL prerequisite. Completion alone is not success.

Selection284 records CK_POWER around the existing typed enable image, then
tests CONTROL0 pair0/2 and bit22 low/high only if ENABLE=VALID=0. It restores
the changed lanes from the owned saved image and records exact restoration
before completion, without a descriptor or TX publication. It then holds FE
frequency updates and strobes inactive, changes the address-high bit
(address0x400) and byte channel index1/255, before restoring the saved fields.
The address-low field is written only0 and reads0 throughout, so it is not
qualified by this experiment. This qualifies address-high/channel-index
composition/readback only.

Record tags: `0x31a0/31b0` arm; `0x3100/01/03/31ff` inherited/prepared/stable/
final CTRL, CLK_CONF and packed LP-clock/reset + SAR-I2C power/reset state;
`0x3102` DAC request/old/new byte; `0x3104` DAC request/readback; `0x3111/15`
elapsed cycles/CTRL/CLK_CONF for DAC11/15; `0x31c0` injection before/after
cycle marks/CLK_CONF. `0x3120` CK_POWER before/after/XOR; `0x3121` saved
CONTROL0/ENABLE/VALID; `0x3122` case/request/readback; `0x3123` saved/restored;
`0x3124/25` address or channel/full FE image; `0x3126` final FE image.

On s31-c rev0, the four cold forced-power arms have a positive DAC response
with READY0 and both opaque CLK_CONF lanes reading0. Fifteen cold injection
arms establish no distinct command, gate or lasting-latch effect. CK_POWER's
requested low4 image15 reads0, while the full enable readback preserves lanes
outside the sourced mask. FE/CONTROL0 results qualify only address-high and
byte-channel composition and disabled CONTROL0 readback described above.
SOURCE: `HW31_S31_*_20261027` in `registers/evidence/hw-analog-pmu.toml`;
`oer-notes/research/hw31-tsens-feest-pmuck.md`.

## Hart 1 ROM entry state

Selections 232/233 release hart 1 once and record the state the rev0 ROM
hands to the boot address. Hart 0 first parks hart 1 (boot word zero, stall
requested, clocks gated, global reset held), records HP core 0/1 reset
causes, `LP_CORE_DISABLE`, `LP_STORE8`, the boot word, hart 1's control and
stall status, `L1_ICACHE_SHUT_IBUS0/1` and the ROM `_putc1`/`_putc2` slots,
and disconnects every nonzero `INTERRUPT_CORE1` route. It then releases hart 1
in ESP-IDF's `start_other_core` order: stall code 0xFF and a bounded stall
status wait, CPU clock, CLIC clock, global reset release. 232 writes the boot
address after that release, so the ROM takes its `main` path; 233 writes it
with bit 31 set before release, which `normal_boot` takes as a direct jump
that skips `main`.

The entry (`src/hart1-entry.s`, internal SRAM) clears MIE with its first
instruction (`csrrci`, keeping the old `mstatus`), installs a recording trap
stub with the inherited `mtvec` mode and stores the CSRs before any other
access: `mstatus`, `mtvec`, `mtvt`, `mscratch`, `MHCR`, `mcounteren`,
`mintthresh`, `mintstatus`, `mhartid`, `mie`, `mip`, the inherited `sp`/`ra`/`gp`,
`mcause`, `pmpcfg0-3`, `pmpaddr0-15`, `pmacfg0-15` and `pmaaddr0-15`. It then
sets `gp` and a 2 KiB SRAM stack and calls Rust, which reads hart 1's own CLIC
view through the session's typed CLIC owners while hart 0 only polls:
`INT_INFO`, `INT_CONFIG`, `INT_THRESH` and every `CTRL` word. Hart 0 waits at
most 40,000,000 cycles for the final stage marker or the trap marker, parks
hart 1 again and repeats its boot-input records.

| Tags (232 / 233) | Observation |
| --- | --- |
| `2320` / `2330` | HP core 0 and 1 reset cause (bits 5:0) and flag (bit 8), `LP_CORE_DISABLE` |
| `2321` / `2331` | `LP_STORE8`, boot word, hart 1 CPU clock/CLIC clock/global reset/force-no-reset/stalled bits 0–4 and stall code in bits 15:8 |
| `2322` / `2332` | `SHUT_IBUS0`/`SHUT_IBUS1` bits 0/1, `_putc1`, `_putc2` |
| `2323` / `2333` | Disconnected nonzero hart-1 routes, first such source, stall-status polls |
| `2324` / `2334` | Hart 1 stage marker, cycles to it, entry address |
| `2325`–`2327` / `2335`–`2337` | The `2320`–`2322` records after hart 1 is parked again |
| `tag + 0x100 + i` | `FACTS_HART1` words `i`, `i+1`, `i+2` |

`FACTS_HART1` word 0 is `0xfac71001` after the CSR snapshot and `0xfac71002`
after the CLIC view. Words 1–15 hold the CSRs and registers above in that
order, 16–19 `pmpcfg`, 20–35 `pmpaddr`, 36–51 `pmacfg`, 52–67 `pmaaddr`, 68
the `mtvec` written by the entry, 70 `INT_INFO` (lines, control bits << 16,
version << 24), 71 `INT_CONFIG` (MNLBITS, NVBITS << 4, NMBITS << 8), 72
`INT_THRESH`, 73 onward one `CTRL` word per line (CTL, mode << 8, IP << 16,
IE << 17, SHV << 18, trigger << 19) and 124–127 the trap marker
`0xfac7100f`, `mcause`, `mepc` and `mtval`.

These leaves observe the ROM download state the loader leaves: no bootloader
ran, so cache buses, printf slots and reset causes differ from a flash boot.
They make no PSRAM or flash access from hart 1 and observe the CLIC only after
the CSR snapshot.

## CLIC levels and the software entry of a level

Selections 310–316 run on hart 0 with hart 1 contained, no RF and no routed
source except where named (`src/image/levels.rs`). Lines are external lines
from 16, hardware-vectored and positive-edge, at level n with control byte
`(n << 5) - 1` (three level bits). A pend is the store of the line's
pending byte as `0x01` (the PAC's `ClicPending::pend_line`, with hart 0's
proof), and a clear its store as `0x00`, with MIE cleared across it; the
store reaches no other byte. Run 187 (lease #1477, 355b7d315) ran these
selections with the earlier pend, a read-modify-write of the line's word;
its record stands for what it ran. The handler logs 16-bit
events `kind << 14 | line << 8 | mintstatus.MIL` (kind 0 enter, 1 exit, 2
mark of a pend after the write, 3 storm guard). Every trial waits 60,000
cycles with MIE set. Records `tag | 0xff` give `(repeat, events, entries)`;
`tag | record << 16 | repeat << 24` carry six events each. The ROM RTC watchdog is fed before every trial.

| Selection | Experiment, ten repeats each |
| --- | --- |
| 310 | Levels 1–7 on lines 16–22, siblings at the same levels on 23–29. The thread pends level 1; level n's handler pends its sibling, level n−1's sibling, then level n+1 |
| 311 | Level 3 (line 32) pended by the thread pends level 1 (line 30) |
| 312 | Level 2 (line 31) pended from the thread, then from level 1's handler |
| 313 | Level 2's handler pends its own line on its first entry |
| 314 | Line 34 pended between line 33 (edge, pended, disabled) and line 35 (FROM_CPU2, level-triggered in variants 0–2, edge in 3–5), all level 2; the source rises before the pend, just before its store, or after it. Run 187's middle variant raised it between the read-modify-write's read and its write; the store has no read, so the variant changed with the store. `0x3500 + v`: `(repeat, entries of 34, 35, 33 in bytes 0–2 and IP of 34, 35, 33 in bits 24–26, all entries)`; `0x3580 + v`/`0x35c0 + v`: the three words before and after the pend |
| 315 | Two pends of line 31 with MIE cleared (`0x3600`: `(repeat, IP, entries)`), then level 3's handler pends level 1 twice (`0x3700` events) |
| 316 | 100 samples: level 3's handler pends level 1 and returns; cycles from the pend's write to level 1's first cycle read (`0x3800` every third sample, `0x38ff`: `(min, max, valid)`) |

These are isolated observations, not a latency bound.

Selection 322 (tasks/253, 2a) is the stored clear of a pended edge line, a
hundred trials of each variant: line 34 at level 3 and line 32 at level 4,
both edge-triggered and enabled. The thread pends line 32. Line 32's handler
stores line 34's pending byte as `0x01`, then in variant 0 as `0x00`, in one
access with MIE cleared; variant 1, the control, only pends. The record is
`(0x3900 + v, trials, held, last)`. A trial holds when line 32 is entered
once and line 34 `v` times, with its pending bit read clear after the
window. `last` is the last trial's `entries of 34 | entries of 32 << 8 |
IP of 34 << 16`.

## A hart's pending store stays its own

Selection 323 (tasks/253, 254) releases hart 1 as 232 does. MIE is
cleared on both harts, and each hart sets up its own line 34 disabled and
positive-edge: a level line's pending bit follows its source, so it would
not hold a store. In a hundred trials each way:

- hart 0 stores its own byte of line 34 pended and reads it set, hart 1
  reads its own clear, and hart 0 clears its own and reads it clear;
- hart 1 stores its own pended through hart 1's copy of the shared part
  (with `OnHart<1>::check`) and reads it set, hart 0 reads its own clear,
  and hart 1 clears its own and reads it clear.

The two harts step in lockstep (`PHASE` from hart 0, `ACK` from hart 1).
Each phase is bounded at 1,000,000 cycles, and hart 1's ready word at the
release's own bound.

| Tag | Words |
| --- | --- |
| `0x3a00` | trials, hart 0's own byte read set, hart 1's own byte read clear while hart 0's was set |
| `0x3a01` | hart 0's own byte read clear after its clear, hart 1's own byte read set, hart 0's own byte read clear while hart 1's was set |
| `0x3a02` | hart 1's own byte read clear after its clear; line 34's state on hart 0 (bits 0–15) and on hart 1 (bits 16–31), each its enable (bit 0) and trigger (bits 1–2) before the setup, and the same after it at bit 8; hart 1's stage (`0xfac71003` done, `0xfac7100e` not hart 1) |
| `0x3a03` | the release: routes found connected on hart 1's bank, the first, stall polls |

Only line 34 and these two harts are observed. Hart 1 takes no entry
here: its vector table and handler come with its own lease.
