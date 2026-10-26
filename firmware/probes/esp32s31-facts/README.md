# ESP32-S31 hardware facts image

This PAC-only image owns the chip once and executes short, bounded register
experiments on hart 0. Hart 1 is held in global reset with its CPU clock gated
before a nonzero boot address is written. There is no HAL dependency and no
production probe API. `src/image.rs::PROBES` is the single registration point.

Build with `cargo fw probe oer-firmware-probes-esp32s31-facts`. Load the ELF
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
observations and the reason tag. The tag identifies the leaf and sample; the
research report records actual values and the stand run ID. The handler stops after eight unexpected entries;
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

## BLE interrupt facts

BLE commands use disjoint PAC owners from the same root claim. They contain
hart 1, keep CPU interrupts masked, observe both MAC banks and matrix status,
and never publish a scheduler descriptor, start scheduler RUN or change a CPU
route. They do not run a radio protocol or require a peer. Production baseline
and arming policy are unaffected.

| Selection | Bounded experiment |
| --- | --- |
| 20 | Bank-one source-1 enable before clock setup, before baseline, and after applying/removing baseline; records clock and enable readbacks |
| 21 | Candidate BTLC cause, baseline masking/re-enable, separate controls 1/2 and their combination |
| 22 | Candidate arrivals between bank snapshots and before sampled acknowledgement |
| 23 | Near compare, software pending, bit-18 disable, retained-pending re-enable and zero acknowledgement |
| 24 | Pending compare, counter progression, `+0x04` image 1, then START and phase observation |
| 25 | One natural counter wrap without compare, bit-18 disable and rollover-byte zero |
| 26 | Half-range compare, all-ones disable, independent bit-1 command, positional modes and a past target |
| 27 | Bank-one source 1 enabled only in the probe for the local BTLC candidates; restores the inherited enable |
| 28 | Counter movement across XTAL divider images 399/1/0 and positional mode writes |
| 29 | Software-pending delivery across mode writes, disable/re-enable, zero, all-ones disable, bit-1 command and `+0x04` |
| 30 | One natural wrap plus software pending; independent suppression, retained-pending redelivery and acknowledgement of the two causes |
| 31 | Minimal sourced MAC/latch prefix and the BTLC candidate, without any DMA or RF publication |
| 32 / 33 | BLE-timer reset with LP clocks on / off; software-pending diagnostic, then one natural rollover with compare disabled |
| 34 / 35 | Production controller-domain reset order with LP clocks on / off; same two positive causes and separate suppression/stop/clear observations |

Reset leaves observe delivery before any post-reset enable/publication. The
software-only phase retains bit 18 enabled as a diagnostic; the rollover phase
disables compare before reset, matching shutdown admission. LP-off deselects
all four LP sources, clears the BLE-timer gate and the MODEM_PWR LP-timer gate,
and keeps the register bus and Wi-Fi-power gate live. Clock restoration sends
neither START nor a pending/enable command. Each state has three records:
`tag` = counter/status/matrix bank 3; `tag+1` = bytes 0x24/0x2c/BLE-reset;
`tag+2` = BLE gate/LP gate/XTAL selector. Tags 0x3220–0x3250 and
0x3340–0x3370 precede all post-reset pending/enable controls; 0x3380 onward separates
all-ones suppression, counter stop, pending clear and positive rearm controls.
There is one bounded natural wrap per command and no direct read of +0x10.

These commands are explicit selections, excluded from selection 0. An inactive
BTLC candidate does not establish an output equation or a set/clear race result:
the records must show a real pending cause before interpreting masking or ack.
The LP counter experiments use divider image 1, which advances the counter in
the tested setup; image 0 did not advance it. They establish no production-clock
frequency equation. Counter/cause polls end after 40,000,000 cycles, and selections 25/30/32–35 stop after one wrap. Multiple-wrap capacity and
collision priority are outside those selections' observations.

BLE leaves feed the existing ROM RTC watchdog through their typed PAC owner
before each experiment; they do not disable it. The named write-protect images
are in the register model. Debugger command dwell is not part of a measured
operation. A reset, zeroed output buffer or untriggered cause is not a passing
hardware claim even if the debugger command itself exits successfully.

## Local radio interrupt retention

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
separately. LP records hold counter/status/matrix3, bytes24/2c/matrix4,
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
reads it and emits no group-plus-`100` post-read observation. Archived HW11
captures used the earlier status/handler/control layout and included those
post-read records; they do not prove a side-effect-free read or read-to-ack.
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

## LP sleep and delivery lanes

Selections 110–114 observe MODEM_PWR sleep state and the known source-127
bank, initially enabling both modeled delivery commands (bits 1/18); the
individual leaves then apply their documented mask/enable phases. No unobserved
bit0/16/17 command at +0x10 is invented from the sleep-command positions.
The absence of such a sourced enable prevents an exhaustive cause-map claim.

| Selection | Bounded experiment |
| --- | --- |
| 110 | Sleep16/+64 crossing under each control0/4 image; persistence, +44 disable, +40 re-enable, +34 all-clear and fresh software delivery without a new enable |
| 111 | One real rollover plus software pending with the counter stopped; all four control0/4 images and each sleep0/16/17 command; delivery mask, separate enable commands, enabled +34 clear and fresh pending |
| 112 | Advancing completed BTLC prerequisite; +68 target before sleep0/17 commands and ETM companions, alone and with a positive sleep16 companion |
| 113 | Sleep0-only +68 crossing, independent sleep17 crossing, +44 retention, +34/zero clear, disabled crossing/late enable and fresh-target control |
| 114 | Near+6c compare under all four control0/4 images, control1 trigger, pending persistence and separate +34 clear |

Each state is four records: counter/cause/matrix bank3; bytes1c/20/24;
byte2c/compact mode58 controls0/2/4/matrix bank4; PMU HP RAW/LP RAW/wake cause. PMU RAW
records contain the modeled fields, read without acknowledgement. Context records add reject and
LP-CPU wake causes, compact wake/reject/LP-wake STATUS and their enables.
Only SOC_WAKEUP and SOC_SLEEP_REJECT (2 of the 5 HP enable fields), and
LP_CPU_WAKEUP (1 of the 12 LP enable fields) are sampled; other enables
are unobserved.
The separate read-only `PmuWakeStatus` capability reads these words; the
radio clock/power owner has no PMU wake enable or acknowledgement access. A change of a
local sleep flag alone does not establish a PMU sleep/wake transition.

Cause/target waits stop after 200,000 cycles. Selection 111 requests exactly
one natural wrap, bounded by 40,000,000 cycles. Persistence windows are
4096 cycles. Selection 110 emits at most 124 records in the 128-record buffer.
CPU interrupts remain masked; no CPU handler delivery is measured. Hart1
stays held. The SRAM loader and debugger capture must run inside an s31-c
stand lease; only command/result SRAM is accessed by the debugger.

CONTROL_1 is a write trigger. All callers use its generated fresh-read
trigger; diagnostic mode encoding omits that write-only lane. Archived
pre-correction captures explicitly include its observed zero readback.

Sleep17/+68 observations use advancing completed BTLC captures, live digital
and ETM clocks, ETM channels6/7 enabled and the ETM bit11 command issued.
None of those prerequisites is independently varied; their necessity and
sufficiency remain unknown. R1/R2 capture only low bytes of +1c/+20 and
mode58 controls0/1/2/4 (controls0/2/4 after the trigger-class correction).
Upper +1c/+20 bits31:8 and mode58 bit3/bits31:5 are not recorded; no claim
that only mode58[2] changes across the complete word follows.

### LP record tags

A state at `t` is four records at `t+0..3`, in the field order above.
Each `pmu_context(p)` is two records: `p` contains reject cause, LP-CPU wake
cause, and compact STATUS (HP wake=1, HP reject=2, LP CPU wake=4); `p+1`
contains the three individually sampled enable booleans in that same order.
Context bases are 0xcff0 (110), 0xdff2 (111), 0xeff0/0xeff2 (112),
0xaff0/0xaff2 (113), and 0x9ff0 (114).

For loop index `i=0..3`, the control0/4 configurations are 0/1/0x10/0x11;
`i` itself is result metadata, not a register image. Leaf tag maps:

| Selection | State bases and phases |
| --- | --- |
| 110 | `b=0xc000+i*0x100`; offsets 0x00 before command, 0x10 after crossing, 0x20 after persistence window, 0x30 after +44 disable16, 0x40 after +40 enable16, 0x50 after +34/window, 0x60 after fresh software pending/window |
| 111 | 0xd000 positive stopped-counter pair; 0xd010/20/30/40 control0/4 indices0/1/2/3; 0xd050/60/70 enable sleep0/16/17; 0xd080/90/a0 disable sleep0/16/17; 0xd0b0 all-delivery mask; 0xd0c0 enable1; 0xd0d0 enable18; 0xd0e0 +34/window; 0xd0f0 fresh software pending/window; 0xd100 zero24/window |
| 112 | `b=0xe000/0xe100` without/with sleep16 companion; offsets 0x00 before commands, 0x10 after crossing, 0x20 after window, 0x30 after sleep0/17/16 disable |
| 113 | 0xa000/0xa010 sleep0-only crossing/disable; 0xa100/110/120/130/140 independent sleep17 crossing, disable17, +34/window, zero20, late enable17/window; 0xa200/210 disabled crossing/late enable17; 0xa300 fresh enabled-target positive control |
| 114 | `b=0x9000+i*0x100`; offsets 0x00 after compare crossing, 0x10 after CONTROL_1 trigger, 0x20 after window, 0x30 after +34 clear |

Selections112/113 prepend the digital BTLC prerequisite's 0x9000–0x90e0
records, followed by LP initialization tags0x2f00/0x2f01. Selection114's
0x9000 bases belong to its own leaf and are not prerequisite records.

`b+0xf0` records `(origin_counter, programmed_target, metadata)`:
metadata is the control0/4 loop index for110/114, the companion16 boolean
for112, and zero for113's 0xa000/0xa100/0xa200/0xa300 target phases.
For112 the target is +68 at origin+1024 and the +64 companion is origin+512;
other target phases use origin+512. Arithmetic wraps in the modeled low
counter width.

`b+0xf1` records `(crossing_budget_passed, sampled_counter, elapsed_cycles)`.
The finite poll watches counter distance >=1024 (110/113/114) or >=2048
(112), under the 200,000-cycle bound. Passing proves counter progress past
the programmed target, not a positive pending flag or IRQ. Those require
the subsequent state records. On timeout the leaf cleans up and returns;
completion alone does not qualify a cause.

Selection111's 0xdff0 instead records `(wrapped, sampled_counter,
elapsed_cycles)` for its one natural-wrap poll under 40,000,000 cycles.
The later positive byte2c check remains an independent precondition.

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

The finite production prefix now follows `hal/src/power.rs::execute_owned`
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
establish a production lifetime contract or qualify PR #707 on their own.
The same ledger is SOURCE `HW17_S31_SAR_PREPARATION_20261027` in the model.

| ID | Difference from production | Reason / consequence |
|---|---|---|
| D1 | ROM RAM-loader state, SRAM stacks and adopted integer-MHz CPU rate; no staged bootstrap/PSRAM, MPLL reference, platform reference counts or Wi-Fi client cold baseline/LP-clock selection/combined BB+MAC reset entry | No Wi-Fi client epoch is opened; a standalone MAC pulse would not reproduce that parent lifecycle. PAC-only image has exclusive words, not the production platform lifecycle. Parent PLL/bootstrap state is inherited and recorded where observable. No production clock/load equivalence is claimed. |
| D2 | SDM checks the same `0x5b` predicate with a 100 µs `mcycle` deadline instead of 9999 MAC-local counter ticks; byte/PBus operations have 20 µs experimental bounds; SAR has a 100 µs bound | Required short stand scope. The ordinary production timer/40 µs policy and vendor unbounded wait are not copied. Named settles preserve ≥100/10/1/2 µs; no executor timing equivalence. |
| D3 | The production background-enable bit after TSENS is deliberately kept 0 before manual reference/tone selection | Prevent autonomous triggers during HOLD/EDGE; retention with background tracking remains untested. |
| D4 | RC settings/calibration, filter DCAP and parameter reads, analog init1, RFPLL charge-pump search, command RAM publication, later conditional SAR2 initial-code write, FE update, crystal-duty and channel initialization are omitted | They belong to the full RF calibration suffix and depend on retained/calibrated parameters. No synthetic substitutes are used. In particular `0x578` is no longer moved before the test conversions. |
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
| 2 | Analog field's read failed | block / byte index (0/0 in the former SAR2-specific image) |
| 3 | Value does not fit the generated analog field | old byte / requested value (former SAR2 path used observation 0) |
| 4 | Analog initialization failed | stage 1=bias, 2=BBPLL, 3=ADC-rate / 0 (former image: SAR2 initialization) |
| 5 | HOLD refused: no admitted nonzero preflight | 0 / 0 |
| 6 | EDGE refused: no distinguishable nonzero references | 0 / 0 |
| 7 | Invalid SAR registry selection | requested selection / 0 |
| 8 | P2 handler did not complete | requested selection / 0 |
| 9 | Common-power readback mismatch | checkpoint / observed boolean |
| a | SDM did not return `0x5b` within the short bound, or transport failed | last byte (`u32::MAX` if unavailable) / elapsed cycles |
| b | PBus-clear command was busy at start or timed out | selector / path |

### Evidence identity

Run `hw17-20261027-sar-review-04` (s31-a, lease #898) loaded the ELF built
by `cargo fw probe` from commit
`17e5ad48e577d8fd80071f7adb0305147e94174c`, published as source tag
[`evidence/hw17-sar-review-04`](https://github.com/ermacv/open-esp-radio/tree/evidence/hw17-sar-review-04).
The measured ELF, exact probe sources, raw JSON/binary dumps and capture
logs for R3/R4 are retained in the single archive
[`hw17-sar-evidence-20261027.tar.gz`](/home/ermacv/.local/share/open-esp-radio/evidence/hw17/hw17-sar-evidence-20261027.tar.gz) in the
[durable evidence directory](/home/ermacv/.local/share/open-esp-radio/evidence/hw17/)
(`<XDG data>/open-esp-radio/evidence/hw17/`). Archive SHA-256:
`671a39bdb8f985fe9cc1b01197b3dcbb3d5e7cf7f9db50887ca4ed8c1e952fa2`.
The archive preserves each run under its run-ID directory and supporting
logs/audits under `support/`. Its contents were compared byte-for-byte with
the captured files before those ELF/JSON/binary scratch captures were moved.
The measured ELF is **not** the final rebased image.
The differences after capture are explicit:

- Rebase onto `a29a0eb3cf55e95b13ef107e15ade12e565a0cab` incorporates
  upstream BLE/local/generic/opaque probes (#683/#686/#689/#691/#692/#697/#699/#701/#702/#703/#682),
  PAC ownership/model/generator updates and watchdog/LP-lane/RX-chain support. These add linked
  code, registry entries, root storage and a generic-IRQ dispatch check;
  the in-progress completion marker and retained LP-reset boot dispatch also
  differ. Image layout and timing are not assumed identical to the capture.
- Shared PHY, chip-clock and boot PAC groups remain owned once in `Session.sar`.
  BLE/local/opaque probe helpers borrow those existing owners rather than claiming
  duplicate handles. All registrations use the same `Action` registry.
  `sar_prepare.rs` remains byte-identical to capture. In `sar.rs`, the
  removed broad host-map setter is replaced by the production named-lane
  setters from #703. Model-based comparison preserves the same `0x3fa0`
  field image and preservation mask; no other SAR operation changes.
- This README, SAR evidence/field descriptions and generated PAC documentation
  annotate the capture. No measurement is relabelled as a final-head run.

SOURCE `HW17_S31_SAR_REVIEW_20261027` records the same distinction. The report
retains the measured/final revisions and exact changed-file inventory. The
rebased image is compile/link/host checked; its added non-SAR probes are not
run by this task.

The corrected-prefix run contains ten conversions (two preflight, four HOLD,
four EDGE), on one board in one short run. HOLD retained data/ready through
the observed 100 µs window. Partial ready codes were nonmonotone. Preflight
reference samples 1969/1963 were unequal, so the gate admitted EDGE, but EDGE
final reference outputs differed by only 1–2 codes, comparable with nearby
conversion variation. This does not establish a robust reference-specific
stimulus or causally attribute the final result difference. The stronger
reference separation in the older incomplete-preparation capture is not
transferred to the corrected prefix or production. No statistical series or
full calibration is added to resolve that limitation.
