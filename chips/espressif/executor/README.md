# Radio execution

[`ieee80211/runtime`](../../../protocols/ieee80211/runtime/) (`oer-ieee80211-runtime`) holds
the chip-independent Wi-Fi execution primitives (the station service, the
lower-MAC owner glue, monitor handoffs), which no chip runtime uses yet. No
Bluetooth LE or IEEE 802.15.4 runtime exists. Directory boundaries describe
the execution responsibility of each package.

## Execution and time contract

A runtime exposes futures and never spawns, names or requires an executor; any
executor that polls them is valid, and nothing below adapters and the
integrating firmware depends on an executor. The portable primitives are
`embassy-sync` (bounded mailboxes and signals over a caller-chosen raw mutex)
and `embassy-futures` (select/join). Time is the [`oer-time`](../../../portable/time/src/lib.rs)
contract: a runtime reads and waits on time only through an `oer_time::Clock`
or `Timer` value that it owns or receives from its caller, never through
`embassy-time`, which only adapters and the integrating firmware use, dev
dependencies included. The integrating firmware passes
`oer_embassy_time::EmbassyClock`, which reads the one monotonic timebase the
final image links; runtime tests pass the per-instance `VirtualClock` or
`SkipClock` of `oer-time-virtual`. A runtime does not assume which executor
wakes its timers. The portable drivers below the runtime (the `service`
packages under [`ieee80211/`](../../../protocols/ieee80211/), such as the station join, scan
and lifecycle drivers and the WPA2 handshake runners) take the same `Timer` port
from the runtime, which polls them. The
protocol state machines they drive never wait.

| Module | Responsibility |
| --- | --- |
| `ieee80211/runtime/src/{monitor,connected_tasks,station_network}` | Portable capture/injection handoffs, task shutdown and association-scoped network ownership |

Hardware transactions and finite chip state remain below these packages. A
runtime retains their affine owners across borrowed waits, returns the same
resources on rejection and preserves terminal owners when quiescence is not
proven. A composed owner alone does not establish hardware behaviour.

The removed BLE and IEEE 802.15.4 runtimes returned `ControlFlow` from their disable sinks: a
refused terminal came back by value and stopped delivery, the runtime
published `TerminalRejected` and finished its hardware stop normally, and
uninstall returned `Uninstalled::TerminalsQuarantined` with only the
undelivered terminals, their queue and pending permits quarantined.

The portable Wi-Fi station service uses `MsduQueue` for application commands
as well as network frames. Each queue has one affine sender and receiver;
waiting for readiness borrows its endpoint exclusively without removing a
value. The service handles a ready port event before taking a command, so a
simultaneous port fault leaves that command owned by its queue.

Owned timers follow the owner graph: each runtime carries the clock of its
own deadlines. The platform executor/time ABI remains in
[adapters](../../../portable/embassy/README.md).

The integrating firmware (none exists yet) chooses memory budgets, claims
static resources and owns the protocol lifecycles within the [documented
capability boundaries](../../esp32s31/FEATURES.md). No firmware depends on a
second runtime owner hidden in a task or network handle.

## Radio interrupt levels

[`executor`](src/lib.rs) (`oer-espressif-executor`) places
the image's contexts on one core: the radio interrupts at P3 (the MAC and
PWR sources and the radio doorbell), the radio executor (`InterruptExecutor`
on `FROM_CPU` line 2) at P2, the application below it. `LevelOrder` checks
when the image compiles that P3 preempts P2 and P2 the application.
esp-hal runs all sources of one level on one core one after another, so the
P3 handlers never preempt each other.

The radio executor never writes the registers a P3 handler owns. It records
a request in memory both sides see and rings the `Doorbell`, a software
interrupt (`FROM_CPU` line 3) at the P3 level: `ring` fences, raises the
interrupt and reads its register back. The doorbell's handler first resets
the interrupt through its owned `DoorbellAcknowledger`, then takes the requests, so a ring after the
take is not lost. Making the doorbell consumes its software interrupt, so no
executor can use the line, and takes a `SameLevel` of the radio interrupt's
and the doorbell's table entries, which checks when the image compiles that
both share one level and one core. The request protocol belongs to the
driver whose handler applies it.

Portable protocol policy cannot depend on this execution domain. The
architecture audit follows transitive normal/build dependencies to enforce
that boundary; portable packages cannot depend on these concrete ESP32-S31
runtimes.

Software-interrupt construction transfers its singleton into separate raise
and reset owners. Wakers borrow the published sender; only the line's handler
borrows its acknowledger. The time driver consumes esp-hal's SYSTIMER into
our comparator writer and three independent W1C alarm acknowledgers. The
writer programs typed comparator indices through `SYSTIMER::regs()`; esp-hal
keeps the independent UNIT0 counter latch and tick rate. Alarm interrupts
never borrow the comparator writer or a task's timer queue.
