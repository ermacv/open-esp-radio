# timer-facts

The host program of `tasks/380` over the probe `esp32s31-timer-facts`:
one case a row of the task's table, and the enables read at the end.

| Case | Passes when |
|---|---|
| `a-loaded-over-a-standing-target` | 1 000 trials each of a later and an earlier target loaded over a standing one: every trial one alarm, at the new target within 1 µs, none before the load |
| `b-loaded-after-an-alarm` | 1 000 trials of a target loaded after an alarm was raised and cleared: one alarm each, at it |
| `c-behind-the-counter` | 1 000 trials each of a target 1, 16, 1 000 and 1 000 000 ticks behind the counter: one alarm each, within 1 µs of the load |
| `d-across-the-standing-expiry` | answered: a load swept across the standing target's expiry over ±5 ticks in quarter ticks, the new target future and behind, 4 trials an offset, each recorded with the offset it had |
| `e-words-written-apart` | 1 000 trials each of `LO` first and `HI` first, the pair passed while one word is written and no load: no alarm before the load, one at the new target after it |
| `f-hart-1-beside` | hart 1 loads comparator 2 in a loop while case a and the sweep of d run on comparator 1: a's counts exact, hart 1's alarms equal to its loads, none missed |
| `g-far-targets` | answered: targets 2^30 to 2^51 ticks ahead and the counter's range less 1 ms |
| `enables-at-the-end` | comparators 1 and 2 still enabled, comparator 1's interrupt enabled and comparator 2's clear |

Every answer is a measure of the results file: the tallies (counts, the
spread of the first alarm's time against its expected cycle, the load's
cycles), d's attempts and the count of future targets that never raised,
g's table, hart 1's counts and the enables. Times are the hart's cycles;
`rate` converts them.

Run it with `cargo hil run timer-facts --board <board>` under a lease.
