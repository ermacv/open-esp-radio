# esp32s31-peer-overdue

The negative check of watched peer requests on hardware: a request whose
client never takes the answer must end supervision with `RequestOverdue`,
retain `PeerUnanswered` and reset, and the next boot must print that record.

The image is a small composition of the staged boot (bootstrap, PSRAM, HAL
launch of hart 1, executors, diagnostics), like `boot-smoke`. Hart 1 runs an
interrupt-mode executor with two tasks: the server of one watched channel
(it answers in place and counts the requests it served) and the liveness
responder. The boot hart runs the RTC watchdog's feed, the liveness
supervisor (a beat every 100 ms, each echoed within 50 ms), which watches the
channel in seat 0 of its obligation table, and the client.

The stand arms one injection: the probe's retained words (`PROBE`, three
words of state, deadline, beats and served requests plus a check word, in
LP RAM) start with `ARMED` (`0x41524d44`) once the stand writes that word
over JTAG. The client polls it every 50 ms. Armed, it stores `INJECTED`
with the request's deadline (300 ms after the post), posts one request and
never calls `answer`. The supervisor's callback counts the beats completed
after the post and the requests hart 1 served into the retained words; at
the deadline the supervisor ends with `RequestOverdue`, records
`PeerUnanswered` (channel 0, the deadline, mark 2) and resets.

The next boot reads both records before the runtime reports and clears its
own. It passes (`OER-RESULT: pass peer-overdue`) only when the boot hart's
reset cause is the software reset (`CoreSoftware`), the runtime record is
exactly `PeerUnanswered` on hart 0 with `[0, deadline low, deadline high,
2]`, at least one beat completed after the post and hart 1 served the
request; any other record or reset cause fails. It then clears its words and
only feeds the watchdog: nothing re-arms the probe but the stand, so it never
enters a reset loop. A flash or a power cycle (`ChipPowerOn`, `Other`) after
an injection discards the stale state.

Run it with `cargo fw run ../probes/esp32s31-peer-overdue --board <board>`
under a lease and, once the console prints `OER-PROBE peer-overdue ready`,
write `ARMED` to `PROBE`'s first word (`llvm-nm` on the built
`runtime.elf` names `PROBE::STORAGE`) through the debug module's system bus
access, without halting a hart.
