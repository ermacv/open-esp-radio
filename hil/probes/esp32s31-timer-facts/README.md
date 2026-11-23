# esp32s31-timer-facts

Whether a SYSTIMER comparator of the ESP32-S31 takes a new target while it
stays enabled: its enable (`CONF.TARGETn_WORK_EN`) and its interrupt enable
(`INT_ENA.TARGETn_INT_ENA`) written once at boot and never after, each
target written as `TARGETn_HI`, `TARGETn_LO` and the `COMPn_LOAD` strobe.

The image is a staged boot with the boot hart's thread executor, the HIL
console and agent, and the executor's own alarm on comparator 0 with its
vendor sequence. The cases run on comparator 1 in the console's commands, on
the boot hart's thread context, each trial with the hart's interrupts
masked. Hart 1 starts during boot, runs no executor and takes no interrupt;
for the concurrent case it loads comparator 2 in a loop, its interrupt
enable left clear, polling the raw status.

An alarm is seen by polling `INT_RAW` against the hart's cycle counter
(`CycleCounter::sample`); it is acknowledged through the comparator's
`SystimerAlarmN` part. The ticks are read on the boot hart alone, from the
counter latch. Each trial starts from a reference alarm and counts its
targets from it (`oer-hil-services-timer-facts`).

## Access

The executor's time driver holds the root's claim of the `Systimer` part.
The probe makes one second claim of it and of the counter latch
(`src/comparators.rs`), approved for `tasks/380`; its `// SAFETY:` holds
the table of who writes which word. The claim's blocks never leave that
module.

| Context | Writes |
|---|---|
| hart 0 thread, the executor | comparator 0's words; `CONF`, `INT_ENA` read-modify-written |
| hart 0 thread, the probe | `TARGET1_CONF`, `TARGET2_CONF`, `CONF`, `INT_ENA` once at boot; `TARGET1_HI`, `TARGET1_LO`, `COMP1_LOAD` |
| hart 1 | `TARGET2_HI`, `TARGET2_LO`, `COMP2_LOAD` |
| each `SystimerAlarmN` part | its own `INT_CLR` bit |

## Endpoints

| Endpoint | Case |
|---|---|
| `timer-facts/calibrate` | the boot's rate: the cycles of 160 000 ticks between two alarms |
| `timer-facts/run` | a (`Later`, `Earlier`), b (`AfterAlarm`), c (`Behind`), e (`Apart`): up to 250 trials, their tally |
| `timer-facts/sweep` | d: up to 8 trials at one asked offset, each as it happened |
| `timer-facts/far` | g: one target far ahead |
| `timer-facts/concurrent` | f: hart 1's loop started or stopped, its counts |
| `timer-facts/words` | the enables, read at the end |

Run it with `cargo hil run timer-facts --board <board>` under a lease.
