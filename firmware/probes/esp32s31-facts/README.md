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
0x3340–0x3370 precede all post-reset control writes; 0x3380 onward separates
all-ones suppression, counter stop, pending clear and positive rearm controls.
There is one bounded natural wrap per command and no direct read of +0x10.

These commands are explicit selections, excluded from selection 0. An inactive
BTLC candidate does not establish an output equation or a set/clear race result:
the records must show a real pending cause before interpreting masking or ack.
The LP counter experiments use divider image 1, which advances the counter in
the tested setup; image 0 did not advance it. They establish no production-clock
frequency equation. Counter/cause polls end after 40,000,000 cycles, and the
wrap leaves stop after one wrap. Multiple-wrap capacity and collision priority
are outside those observations.

BLE leaves feed the existing ROM RTC watchdog through their typed PAC owner
before each experiment; they do not disable it. The named write-protect images
are in the register model. Debugger command dwell is not part of a measured
operation. A reset, zeroed output buffer or untriggered cause is not a passing
hardware claim even if the debugger command itself exits successfully.
