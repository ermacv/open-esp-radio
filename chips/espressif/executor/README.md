# Espressif executor

`oer-espressif-executor` runs statically placed tasks over the platform port.
The image selects its chip binding, contexts, task set and timers. Protocol
runtimes expose futures; images choose which executor polls them.

## Tasks

Each executor's tasks are declared once in the image, by `tasks!`, as a set
bound to that executor by type (`InterruptExecutor<B, N>` or `Executor<B, N>`
of the chip binding `B`).
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
`task::poll_task`, which the declaration exports as a `#[used]` record,
`__OER_TASK_POLL.<module path>`, for the image checks. Both executors
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
`static` waker vtable and the Embassy time driver's `_embassy_time_now` and
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
reads the line back. The time driver consumes `SystemTimer`, containing
the running timer's disjoint owners from the reset root: our comparator writer, three alarm
acknowledgers that each strobe their own bit of the shared
acknowledgement block, and the shared counter latch, which every context
reads through `oer-espressif-boot::drivers::system_timer`: strobe, 64 bounded
VALID polls per settle, then up to 16 LO/HI/LO rounds, retaining a
monotonic floor when preempting strobes prevent verification. The binding
requires the family driver's latch atoms on the platform port's counter owner;
HAL performs no retry. S31 selects 16 ticks per microsecond. Alarm
interrupts never borrow the comparator writer or a task's timer queue.
Successful `init` returns the affine, hart-local `TimeDriverReady<B>` authority;
each additional `bind` borrows it. A timer binding cannot precede initialization.
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

`init`, `bind`, `ComparatorWriter::adopt`, `InterruptExecutor::start` and
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
