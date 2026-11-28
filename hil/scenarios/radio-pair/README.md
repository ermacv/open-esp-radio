# The radio pair

`oer-hil-scenarios-radio-pair` runs two ESP32-S31 boards under one lease,
with the air exclusive. The receiver runs
[`esp32s31-rx-ring`](../../probes/esp32s31-rx-ring/README.md) and the sender
[`esp32s31-case-c-sender`](../../probes/esp32s31-case-c-sender/README.md).
Each board's bring-up answers its station address, and the host tells each
board its peer's. Every step is a request the device acknowledges
(`oer-hil-services-radio-pair`); the cases and the verdict are this
program's.

```console
cargo hil run radio-pair --board receiver=s31-a --board sender=s31-b
```

| Case | Steps | Pass |
|---|---|---|
| `cold-snapshots` | each board's snapshot before its bring-up: the reset cause and the radio's power, clock and reset words as the previous run left them | both answer every page |
| `bring-up` | each board brought up (PHY, power, clocks, the MAC's cold start on its ring, channel 13 at 20 MHz), then its snapshot after it and its receive counters | both up |
| `peer` | the sender told the receiver's address (`target`), the receiver the sender's (`expect`) | both acknowledge |
| `hand-over` | the receiver's `hand-over`, a command that does no work, 64 times with the radio up and the sender silent | every round answered with its spans; the least, the median and the greatest of each span (`down`, `held`, `up`, in the hart's cycles) and the adopted rate's cycles of a microsecond kept as measures |
| `case-c` | R0, a 100 ms window that empties the ring the air filled since the bring-up; the sender's 10 000 rounds of case C posted (about 75 s, past R6's end); meanwhile the receiver's R1 and R2 (a 2 s window, chains recorded), R3 (a 100 ms stall, the walker's standing, a 1 s window), R4 (50 turns of a 200 ms window, the stop, the cold start, a 200 ms window) and R5 (R4's turns with the PHY's close and wake of RF between the stop and the cold start, and the tracking pass when one is due) and R6 (10 of R5's turns, each with a tracking pass whose calibration threshold is zero, so that its common and transmit branches and their power detector's conversions run); the counters after R2, R4, R5 and R6; then the sender's ends | each R's thresholds (`tasks/277`, `281`) and the rounds without a failure; a failing case names every R that failed, and the receiver's step that ran when the sender's rounds failed; both boards' snapshots and counters are then kept again |
| `stop` | each board's stop: the walker's edge, the stop request and its hold, the reset after a stop, the release | both stopped |
| `first-frame` | for each length of 100, 252, 253 and 700 octets, three fresh boots of the receiver, each with one `single` frame as its first frame (below) | every complete boot: the frame's one terminal descriptor where its length puts it from d0, no length anomaly, and one unit of the sender's pattern of its length (rounded up to a word) |
| `ring-full` | five fresh boots of the receiver (up to eight), each: the ring written whole, its tail too, by 32 of the sender's broadcast Probe Requests back to back (an SSID no access point has); the counters; then a 500 ms window under 200 Data frames to the receiver | every boot that wrote the tail (LAST the last descriptor, NEXT zero, its word completed): the window gives the sender's frames, as many as the receiver acknowledged, and no anomaly; a boot that did not write it is repeated, and fewer than five is incomplete (`tasks/444`) |

`first-frame` is the regression case of a first frame that began outside
the ring and ended in it (`tasks/358`); it runs last, since it boots both
boards afresh. Each boot:
1. the receiver reset, brought up and told its sender;
2. its counters, every descriptor armed (else the boot is repeated);
3. the sender's `single` frame of that length;
4. the counters again: every descriptor that is not armed lies among the
   descriptors one frame of that length takes from d0 (192 octets in d0,
   256 in each after it), and the sender named the frame's end (else
   repeated);
5. a 1 s window;
6. the stop.

A length may take eight boots. Each complete boot is kept in
`receiver/first-frame` with what the fixed ring leaves beside it; a
complete boot that is not that fails the case, and a length short of its
boots leaves it incomplete.

The thresholds: R1 at least 1 000 sender units, none mismatched, no length
disagreement, no anomaly, the 16 spares held; R2 at least 200 chains of each
of the 700- and 1 500-octet cases, none mismatched; R3 the cursor steady in
one of the two recorded states and at least 100 sender units after; R4, R5
and R6 every turn the 32 handles back, at least 50 sender units after and some
before, no mismatch, no anomaly, and RF kept (R4), closed and woken (R5) or
closed, woken and tracked (R6), R6's pass with at least one conversion and
every one read ready.

## Measurements and the compare

Each measurement is kept in the run's results file
(`target/hil/results/radio-pair-<time>.json`) under `<role>/<name>`: the
snapshots (`snapshot-cold`, `snapshot-up`), the counters (`counters-up`,
`counters-r1-r2`, `counters-r4`, `counters-r5`, `counters-r6`, `counters-rounds`), every window, the
standing, the turns (`r4-turns`, `r5-turns`, `r6-turns`: each with its stop's and start's
microseconds, R5's and R6's with their close's and wake's, R6's with its
pass's microseconds and its conversions: how many were read ready and unready,
the earliest and the latest a ready one was read after its 2 µs settle, and
the most observations one made), the rounds' ends, the stops' microseconds and each board's
registration's microseconds (`register-micros`) and waits (`register-waits`:
by kind, the spun and the timed settles, the completion, bus-busy and engine
waits and the yields, each with its count and the microseconds asked and
taken). The
receiver's steps are kept with their times since the rounds were posted
(`receiver/steps`); a failed series keeps `sender/rounds-failure` (the
moment and the receiver's step then), and after any failed case
`snapshot-after-failure` and `counters-after-failure` of both boards.

```console
cargo run -p oer-hil-scenarios-radio-pair --bin radio-pair-compare -- A.json B.json
```

prints every word in which two runs differ: a snapshot's readings by item and
index, every other measurement leaf by leaf. It is meant for a deaf boot's
results against a hearing boot's of the same images.
