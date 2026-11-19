# esp32s31-level-wake

A level task's wake on the boot hart, through the product's own parts:
- the image's `interrupt_table!` with a `LevelTask` entry;
- the HAL's controller, `Router::bind` and `verify`;
- the task's `Pend` over the line's handle from the hart's `PendLines`;
- the entry's witness (`Here`);
- `OnHart`.

Hart 1 stays held, and there is no radio. The probe is the mechanism's
consumer until the keeper is.

The levels are the application's grid:

| Level | Entry |
| --- | --- |
| P1 | the thread-mode executor on `FROM_CPU0` (line 16) and its alarm (line 17): the RTC watchdog's feed and the report |
| P2 | an interrupt-mode executor on `FROM_CPU2` (line 18): its task runs the trials and pends the task from below |
| P3 | the level task on line 34, a software line of its own |
| P4 | a `Hardware` entry on `FROM_CPU3` (line 19): it lowers its line and pends the task with its witness |

"Below" is a task of the P2 executor, not the thread: `Pend` names its task
raiser by an `ExecutorEntry`, and the thread executor's wake is a
`Hardware` entry.

A bind is once a boot: the entry's storage is claimed once.

| Selection | What one boot checks |
| --- | --- |
| 1 (default build) | the plain bind returns its proof; `verify` after it passes (the enable, the attributes, the level); the task is not entered after the enable |
| 2b (`--features pended-bind`) | the task's pending byte stored pended through the line's handle with the hart's proof before the bind; then as 1: the bind's clear leaves no entry after the enable |
| 3 | a hundred trials: the handler above pends the task through `Pend::from_handler`; one entry, after the handler's return |
| 4 | a hundred trials: the P2 task pends through `Pend::from_task` with `OnHart::check`'s proof; one entry |
| 5 | a hundred trials: the handler above pends twice; one entry |
| 6 | a hundred trials: the P2 task pends; the task's first entry raises the handler above, which pends it while it runs; one more entry, after the first returns |
| 7 | the least and the most of 3's and 4's spans, in cycles: from `mcycle` read just before the store to the task's first statement (its prologue included). Isolated figures, not bounds |
| 8 | `verify` again once the trials end |

Selection 2a, the stored clear of a pended line, is not here: the pend
lines give no clear. It is the facts probe's selection 322.

The fatal path's read of a task line's pending byte has no stand
selection. The trap records a line's word only for a line no handler takes,
and a task line has its handler; its read stays with the HAL's host test
`the_fatal_view_reads_exactly_the_routes_and_lines_the_words_hold`.

Each trial waits 20,000 cycles for its entries. The report prints after the
console attaches (5 s): one line a selection, then `OER-RESULT: pass
level-wake` when every selection held in every trial.

Run it under a lease:

```console
cargo fw run esp32s31-level-wake --board <board>
cargo fw run esp32s31-level-wake --board <board> --features pended-bind
```
