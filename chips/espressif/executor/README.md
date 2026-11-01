# Espressif executor

`oer-espressif-executor` runs statically placed tasks over the platform port.
The image selects its chip binding, contexts, task set and timers. Protocol
runtimes expose futures; images choose which executor polls them.

## Tasks

Each executor's tasks are declared by `tasks!`, as a set bound to that
executor by type (`InterruptExecutor<B, N>` or `Executor<B, N>` of the chip
binding `B`). A set may extend one other set of the same executor and mode
(`App: Executor<B, 0> extends Library { … }`): the base's tasks keep their
indices, the extending set's follow them, and running the executor with the
extending set polls both through direct calls. `Spawner::base` gives the
base's start right from the extending set's spawner, so a library declares
its service tasks once and an image adds its own after them. A base on
another executor fails const evaluation and one of another binding or mode
fails the `Extends` bound.
Each task lives in a static slot of its own type; a start writes the task's
start (its arguments and how to make its future) in place, once: a second
start is refused (`AlreadyStarted`). The executor makes the future in place
at the task's first poll, in its own context, and the future never moves.
The executor keeps one ready bit per task (at most `MAX_TASKS`), polls each
task ready when a round starts once, by a direct call per task, and drops a
finished task's future in place. A task woken during a round waits for the
next one, after the executor's timers and signals. An interrupt-mode
executor's tasks may start from a lower context: the start claims the slot,
writes the start and only then sets its bit; only the start crosses
contexts, so it is `Send`, and the future need not be. Every waker has one vtable, the binding's, whose data is the executor and the task's
index, never a pointer; the timer queue keeps one deadline per task by that
index. Each task's poll is one direct call of its never-inlined instance of
`task::poll_task`, whose zero-sized first parameter, `task::PollBoundary`,
marks it in the DWARF for the image checks; a task set whose executor the
image never runs links no boundary and none of its tasks' code. Both executors
implement `SignalExecutor`, so an `IrqSignal` wakes a task of either by its
`TaskId`. An executor's context is the executor that runs on the core and
the interrupt level it entered at (`mintstatus.mil`): a hardware interrupt
that preempts it runs at a higher level and is never its context, so it gets
no signal context and no timer slot. The executor's calls into the chip
(software interrupt lines, the wait for an interrupt, the interrupt level,
the core, the system timer's counter, the pending sources) sit in
`chip.rs`; architectural instructions come from `oer-riscv`. The executor
names no chip: its code is generic over a chip binding `B: Binding`, whose
`Port` is the chip HAL's implementation of the platform port's
`ExecutorPlatform` and `Platform` (`oer-espressif-platform-port`) for source
IDs, pending and debugger reads, the software lines, the counter, the
comparators and the fatal path; the hart geometry is the port's. Rust has no
generic statics, so the chip's composition crate declares the binding once
with `bind!(Esp32s31, Port)` (`chips/esp32s31/compose`): the `Storage` of the
port's published lines, counter latch and comparator writer, the one
`static` waker vtable. The image declares its time driver with `time_driver!`,
which exports the Embassy time driver's `_embassy_time_now` and
`_embassy_time_schedule_wake` symbols. The signal lists and memory contexts
(`ThreadContext<SWI>`, `InterruptContext<SWI>`) name no binding, since an
image links one.

## Radio interrupt levels

[`executor`](src/lib.rs) (`oer-espressif-executor`) places
the image's contexts on one core: the radio interrupts at P3 (the MAC and
PWR sources and the radio doorbell), the radio executor (`InterruptExecutor`
on `FROM_CPU` line 2) at P2, the application below it. The table declares
named levels once. Signal declarations check each handler above its receiving
executor, or each raising task below it. Request bindings check the receiving
handler above the requesting executor, on the same hart.
Equal CLIC levels on one hart cannot preempt each other.

The radio executor never writes the registers a P3 handler owns. It records
a request in memory both sides see and rings the `Doorbell`, a software
interrupt (`FROM_CPU` line 3) that is one source of the radio handler's part:
`ring` fences and raises the line. `DoorbellLine::split` consumes the software
interrupt, so no executor can use the line, and hands out the
`DoorbellAcknowledger` for that part and the unbound `DoorbellLine`; only
`Doorbell::<B, SWI, HandlerEntry, ExecutorEntry>::new::<Member>(line, &bound)` makes the
ringing half, once the router bound a part whose sources hold the line's
(`Member::SOURCE` in `HandlerEntry::Part::SOURCES`, checked when the image compiles).
The same constructor checks that the handler preempts the requesting executor
and that the requester has class `Executor`. Both entries remain in the
doorbell type; wrappers derive their metadata from it.
A part of the line alone is the acknowledger itself, which names the line's
source. The handler first lowers the interrupt through the acknowledger,
then takes the requests, so a ring after the take is not lost. The request
protocol belongs to the driver whose handler applies it.

Portable protocol policy cannot depend on this execution domain. The
architecture audit follows transitive normal/build dependencies to enforce
that boundary; portable packages cannot depend on these concrete ESP32-S31
runtimes.

Constructing an executor or the doorbell consumes its line's typed block (`FROM_CPU_<N>`, a
shared block: every context raises it through `&self`). Wakers raise the
published line; only the line's handler lowers it, and it lowers it before it
takes the work, so a raise during the work runs another round. A raise
reads the line back. Every raise (`raise_interrupt`, `Doorbell::ring`)
fences `w, o` after the raiser's published facts and before the device
write, and every handler round (`serve`, the acknowledgers) lowers, fences
`o, rw` and only then takes the facts: a memory fence alone orders no MMIO
access. Across harts this order is conditional on two hardware
preconditions the repository does not establish: the core honors the I/O
bits of FENCE and the two harts' writes of one `HP_CPU_INT_FROM_CPU_n` word
are serialized (H1), and a level last written 1 eventually pends on the
receiving hart (H2).

## Harts

Each executor starts with its hart's `Hart<H>` (from that hart's interrupt
enable) and const-checks that its table entry names hart `H`; an executor
runs on any hart that has its own router, including a secondary hart's
interrupt-mode executor on its own `FROM_CPU` line routed to that hart's
bank. Each executor's running context, ready mask and task slots are its
own, and its task wakers stay on its hart. A task on one hart reaches a
task on another only by raising the other executor's `IrqSignal`, declared
with a `FromPeerHart` raiser. [`peer`](src/peer.rs) carries one request and
its answer between two such tasks over an `oer-memory` in-place request
slot: a successful `post` creates the obligation of one answer by the
deadline it names, which the client keeps across cancelled waits; the slot
is `WORKING` only inside the server's call. The client returns an answer
only when its own executor's timer, read after taking it, is still before
the deadline; otherwise it ends in the terminal `PeerError::Expired` and
stays quarantined: a late answer is never returned and nothing more posts.
`PeerServer::serve_one` answers one request; work that waits (an AES
operation on the server's timers) runs between two answers, on what the
first one handed over, never inside the in-place call.

[`peer::liveness`](src/peer/liveness.rs) is the durable supervision of a
peer hart: the image declares a static seat, which `liveness::pair` claims
once, and a never-ending supervisor task that owns it and its executor's
timer. Every stated `Period` it posts a `Beat`, which the `Responder` task
echoes within the stated deadline; an echo not taken before the deadline
ends supervision with `LivenessError::Overdue` (its sequence, deadline and completed rounds), which
the image records (`LivenessOverdue`) and resets on. A completed round
proves that the responder's hart runs its executor and polls the responder
and that both signals cross; it proves nothing about other drivers there.
When the supervisor's executor stops, nothing supervises: the RTC watchdog
that executor feeds remains the bound.

The supervisor also watches the requests of its executor's clients
([`peer::obligation`](src/peer/obligation.rs)). It owns a static
`Obligations<N>` table, claimed once, and issues one seat per channel
(`Supervisor::watch`), which `peer::pair` requires. The client publishes the
request's deadline into its seat before the server sees the request and
withdraws it only for an answer taken before the deadline; an expiry, a late
answer, or a wait abandoned or never started leave it there. The supervisor
wakes at the earliest watched deadline and at every seat write, during the
beat's wait as between rounds, and ends with
`LivenessError::RequestOverdue` (channel and deadline), which the image
records (`PeerUnanswered`, last word 2) and resets on. Executor locality is
checked: `post` and the discharge take the proof that the caller runs in
the client executor's context (`signal::enter`), and the supervisor
registers its task only after proving that one of that executor's tasks
polls (`signal::enter_task`); outside it, `post` returns
`PostError::Foreign` before publishing anything. A seat keeps its posted
flag apart from the deadline, so every `Instant` is watched unchanged; the
64-bit deadline is read whole or skipped and read again after the writer's
wake.

The time driver consumes `SystemTimer`, containing
the running timer's disjoint owners from the reset root: our comparator writer, three alarm
acknowledgers that each strobe their own bit of the shared
acknowledgement block, and the shared counter latch, which every context
of every hart reads through `oer-espressif-boot::drivers::system_timer`: strobe, 64 bounded
VALID polls per settle, then up to 16 LO/HI/LO rounds, retaining a
monotonic floor when other strobes (a preempting context's, or another
hart's between any two accesses) prevent verification. The binding
requires the family driver's latch atoms on the platform port's counter owner;
HAL performs no retry. S31 selects 16 ticks per microsecond. Alarm
interrupts never borrow the comparator writer or a task's timer queue.
Successful `init` returns the affine, hart-local `TimeDriverReady<B>` authority;
each additional `bind` borrows it. A timer binding cannot precede initialization.
An executor on another hart gets its comparator through `grant`, which
borrows the same authority on the writer's hart, publishes the assignment
and binds the comparator in the writer's bank, and returns an affine,
`Send` `AlarmGrant<B, SWI, Entry>`; on the alarm entry's hart,
`AlarmGrant::bind(&Hart<H>, router)` checks the entry's hart against `H` at
compile time and routes the alarm with that hart's router. The alarm's
handler there acknowledges only its own bit; the writer remains the one
writer of the SYSTIMER configuration. That executor's timer latency past a
deadline adds, as stated parts and not measured numbers, the cross-hart
raise of its publication, the writer's preemption on its hart and the
writer executor's round in progress, since the writer programs published
deadlines when it serves its timers at the start of a round. The time
driver's statics that contexts of different harts touch, the executors'
`CURRENT` and interrupt-mode flags and the binding's storage sit in critical
SRAM (`placed_static!`).
Software line identities use `SoftwareInterrupt`, and comparator selection uses
the platform's `Comparator`; fixed source numbers are checked in const contexts.

The interrupt table uses numeric levels and harts and each part's
`InterruptSource`, with no vendor enum. Executor start and timer binding
consume the table token and interrupt part through `Router::bind`. Doorbell
setup returns its acknowledger before the image binds the handler's part,
and its ringing half only after.
Constructors consume owned HAL parts from the reset root; only the PAC root
claims hardware.

## Setup and invariant failures

`init`, `bind`, `AlarmGrant::bind`, `ComparatorWriter::adopt`, `InterruptExecutor::start` and
`Executor::run` return `Result` for counter-latch and routing refusals. A
`Failure<P, T>` retains its inputs in quarantine even when dropped; the image
calls its never-returning `reset` to record the named `Fault` through the
staged runtime's `fatal::reset_recording` on platform `P`. Successful thread execution has
`Infallible` as its return value. Every firmware caller handles the refusal.

Duplicate publication, missing comparator ownership and foreign cooperative
context are named faults. No task, handler, comparator or
line owner is released on their terminal path. Four diagnostic words and the
reason reuse the runtime's retained panic slot; the next boot prints them,
retaining the record if console output is refused.

`SignalExecutor` is a safe scheduling contract. Each executor's context
type's `ExecutionContext` implementation proves exclusive execution on its
claimed hart, excluding preempting handlers; an architectural ID that names
no hart is no executor's context. Checked `Entered` proofs admit its
signal cells; each software line and executor mode has its own typed static
signal list. Signal task handles are `Send` values, while mutable payload
access remains confined to the receiving execution context.

## Trace observations

`tasks!` accepts `@observer: image::Observer;` before its task declarations;
without that entry the observer is `Noop`. `Tasks::observer` supplies the
image-selected value for each boundary, without an executor counter. Under `oer_trace`, `observation::Activity` reports work and
idle boundaries with hart, executor, raw CLIC level and wrapping platform
monotonic microseconds. The event uses platform kind 40, channel 7. No factory,
clock or context read is compiled into these points without `oer_trace`.

Work brackets timer/signal service and the entire poll round, including
preemption. Work spans of different executors overlap. Idle brackets WFI with
interrupts masked; both edges are reported after the wait and before the waking
handler runs, excluding observer work. A debugger-skipped WFI reports no idle.
The image owns counters and computes whole-hart busy time as `window - idle`;
it never sums inclusive work spans. The HIL station's `measurements` collector
samples on its own hart and refuses windows of zero or at least 2^31 us
(about 35 minutes). Timestamp and cumulative-counter differences wrap. This
information has no scheduling policy or acceptance threshold.

Timer events use the same observer and cfg. Each image declares its Embassy
time driver with `time_driver!(Binding)` (default `Noop`) or
`time_driver!(Binding, image::Observer)` for a unit-struct observer. Alarm
handlers name their receiving task set. All observer calls are static, with
no callback table. The executor retains no measurement totals or window state.
The PHY diagnostic image owns the timer report collector and its window guard.
Uninstrumented builds contain no observation clock reads or observer state.
