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
ROM-loader entry after software reset, without another reset. Selections:

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
then four-word records `(tag, a, b, c)`. Marker `0xfac70001` means completed;
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

## SAR/PWDET lifetime and freshness

Selections 140–146 and 149 use the same one-time PAC claim. Run 140 once
before the others. It applies a finite subset of the production modem clock,
FE/BB, PWDET and SAR2 initialization leaves, selects the existing reference
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
| 149 | Stop both tone paths and restore DAC scale; the host then uses the stand's download reset to end the epoch |

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
