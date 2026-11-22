# The radio pair

`oer-hil-scenarios-radio-pair` runs two ESP32-S31 boards and the stand's
injector under one lease. The receiver runs
[`esp32s31-rx-ring`](../../probes/esp32s31-rx-ring/README.md) and the sender
[`esp32s31-case-c-sender`](../../probes/esp32s31-case-c-sender/README.md);
the injector is the ESP32-C5 running the stand's injector firmware
(`tools/lab/peers/esp32c5-wifi-injector`). Every step is a request the
device acknowledges (`oer-hil-services-radio-pair`); the cases and the
verdict are this program's.

```console
cargo hil run radio-pair --board receiver=s31-a --board sender=s31-b --injector c5-a
```

| Case | Steps | Pass |
|---|---|---|
| `cold-snapshots` | each board's snapshot before its bring-up: the reset cause and what the previous run left | both answer every page |
| `bring-up` | each board brought up (PHY, power, clocks, the MAC's cold start on its ring, channel 13 at 20 MHz), then its snapshot after it and its receive counters | both up |
| `peer` | the injector's `SYNC` and `MODE 13 20 6m`; the sender listens; the injector's 300 ms burst to the sender's station from the receiver's; the sender asked for its peer for up to 3 s; the receiver told the sender's address | the sender's peer is the receiver |
| `case-c` | the sender's 12 000 rounds of case C posted; meanwhile the receiver's R1 and R2 (a 2 s window, chains recorded), R3 (a 100 ms stall, the walker's standing, a 1 s window) and R4 (50 turns of a 200 ms window, the stop, the cold start, a 200 ms window); the counters after R2 and R4; then the sender's ends | each R's thresholds (`tasks/277`, `281`) and the rounds without a failure; a failing case names every R that failed |
| `stop` | each board's stop: the walker's edge, the stop request and its hold, the reset after a stop, the release | both stopped |

The thresholds: R1 at least 1 000 sender units, none mismatched, no length
disagreement, no anomaly, the 15 spares held; R2 at least 200 chains of each
of the 700- and 1 500-octet cases, none mismatched; R3 the cursor steady in
one of the two recorded states and at least 100 sender units after; R4 every
turn the 32 handles back, at least 50 sender units after and some before, no
mismatch, no anomaly.

## Measurements and the compare

Each measurement is kept in the run's results file
(`target/hil/results/radio-pair-<time>.json`) under `<role>/<name>`: the
snapshots (`snapshot-cold`, `snapshot-up`), the counters (`counters-up`,
`counters-r1-r2`, `counters-r4`, `counters-rounds`), every window, the
standing, the turns, the rounds' ends and the stops' microseconds.

```console
cargo run -p oer-hil-scenarios-radio-pair --bin radio-pair-compare -- A.json B.json
```

prints every word in which two runs differ: a snapshot's readings by item and
index, every other measurement leaf by leaf. It is meant for a deaf boot's
results against a hearing boot's of the same images.
