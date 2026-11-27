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
| `case-c` | the sender's 10 000 rounds of case C posted (about 75 s, past R5's end); meanwhile the receiver's R1 and R2 (a 2 s window, chains recorded), R3 (a 100 ms stall, the walker's standing, a 1 s window), R4 (50 turns of a 200 ms window, the stop, the cold start, a 200 ms window) and R5 (R4's turns with the PHY's close and wake of RF between the stop and the cold start, and the tracking pass when one is due); the counters after R2, R4 and R5; then the sender's ends | each R's thresholds (`tasks/277`, `281`) and the rounds without a failure; a failing case names every R that failed, and the receiver's step that ran when the sender's rounds failed; both boards' snapshots and counters are then kept again |
| `ring-full` | the sender's 6 000 rounds of case C posted; meanwhile 20 turns of each take on the receiver, each a stop and a cold start, the ring unserved for 100 ms, the walker's standing, one service of the take (every unit; every unit but the one at the ring's last descriptor; that, the cursor read until NEXT leaves zero within 1 ms, and a second service of the rest), the standing after each, then a 200 ms window; the counters after each take | every turn answers; the ring's outcome is recorded, not judged (`tasks/444`) |
| `stop` | each board's stop: the walker's edge, the stop request and its hold, the reset after a stop, the release | both stopped |
| `first-frame` | for each length of 100, 252, 253 and 700 octets, three fresh boots of the receiver, each with one `single` frame as its first frame (below) | every complete boot: the frame's one terminal descriptor where its length puts it from d0, no length anomaly, and one unit of the sender's pattern of its length (rounded up to a word) |
| `ring-boots` | for each kind of air (the sender's broadcast Probe Requests of an SSID no access point has, 50 ms apart; the same, back to back), five fresh boots of the receiver: the bring-up, 32 frames with the ring unserved, the receive counters (the cursor and every descriptor's word), the first service with each unit's descriptors, length and first 22 octets, then a 500 ms window under 200 frames to the receiver | every step answers; the ring's outcome is recorded, not judged (`tasks/444`) |
| `ring-exits` | for each way out (one service of the whole ring; one of all but the last descriptor's unit and its doorbell, the cursor read until NEXT leaves zero within 2 ms, then a service of the rest; a whole service, then `RX_DESCRIPTOR_BASE` written by the probe with the first re-armed descriptor, the doorbell rung or not), five fresh boots of the receiver, the ring written whole by 32 of the sender's Probe Requests back to back, the way on a quiet air, then a 500 ms window under 200 frames to the receiver | every step answers; the outcome is recorded, not judged (`tasks/444`) |

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
one of the two recorded states and at least 100 sender units after; R4 and R5
every turn the 32 handles back, at least 50 sender units after and some before,
no mismatch, no anomaly, and RF kept (R4) or closed and woken (R5).

## Measurements and the compare

Each measurement is kept in the run's results file
(`target/hil/results/radio-pair-<time>.json`) under `<role>/<name>`: the
snapshots (`snapshot-cold`, `snapshot-up`), the counters (`counters-up`,
`counters-r1-r2`, `counters-r4`, `counters-r5`, `counters-rounds`), every window, the
standing, the turns (`r4-turns`, `r5-turns`: each with its stop's and start's
microseconds, and R5's with its close's and wake's), the rounds' ends, the stops' microseconds and each board's
registration's microseconds (`register-micros`). The
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
