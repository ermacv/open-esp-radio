# timer-facts

The host program of `tasks/380` and `tasks/381` over the probe
`esp32s31-timer-facts`: one case a row of the tasks' tables, and the
enables read at the end. Every trial writes its target's words before its
act and strobes the load at the act; offsets are counted from the strobe.

| Case | What it runs | Passes when |
|---|---|---|
| `a-loaded-over-a-standing-target` | 1 000 trials each of a later and an earlier target strobed over a standing one | every trial one alarm, at the new target within 1 µs, none before the strobe |
| `b-loaded-after-an-alarm` | 1 000 trials of a target strobed over the reference, reached and acknowledged | the same |
| `c2-behind-the-counter` | 1 000 trials each of a target 9, 16, 1 000 and 1 000 000 ticks behind the counter at the strobe, over the reached reference and over a target still ahead | answered |
| `d2-strobe-across-the-standing-expiry` | the strobe swept from 10 ticks before the standing target's expiry to 10 after in quarter ticks, the new target future and behind, 8 trials an offset, each with the strobe's offset it had | answered, and no future target left without an alarm at its time |
| `e2-the-high-word-changed-by-the-load` | 1 000 trials each: the mixed value (new high, old low) passed or ahead, the high word written first or the low; and the passed one with both words written long before and the strobe alone | answered, and every trial an alarm at the new target |
| `f-hart-1-beside` | hart 1 loads comparator 2 in a loop while a and the sweep of d′ run on comparator 1 | a's counts exact, hart 1's alarms equal to its loads, none missed, no future target of d′ left without an alarm |
| `g2-far-targets-around-the-half` | over a target still ahead, targets 2^51 − 16, 2^51, 2^51 + 16 and 2^51 + 2^40 ticks ahead | answered |
| `h-the-short-timer` | over the reached reference and over a target still ahead, a target 10 ticks behind to 10 ahead of the counter at the strobe in quarter ticks, one read of the counter after the strobe, then the watch; 8 trials a distance | answered, and no trial without an alarm whose read still had the target ahead |
| `enables-at-the-end` | the enables | comparators 1 and 2 enabled, comparator 1's interrupt enabled and comparator 2's clear |

Every answer is a measure of the results file: the tallies (trials, alarms
before and after the strobe, early ones, those at the expected cycle and
their spread), d′'s attempts, h's trials, g′'s table, hart 1's counts and
the enables, with the two refuting counts (`*/future-without-alarm`,
`h/refuting`). Times are the hart's cycles; `rate` converts them.

Run it with `cargo hil run timer-facts --board <board>` under a lease.
