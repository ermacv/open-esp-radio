# oer-chips-family-platform-interrupt-table

Espressif interrupt-matrix bindings and vector ABI, including the symbols
consumed by the staged runtime. A chip's controller binds a line at a level
(`Control`, from `oer_portable_riscv::clic`), disables it, and checks it
against the table's `LineExpectation`; the trigger, enable and vectoring of a
bound line are the chip's, and a line it finds otherwise is reported with its
`LineObservation`, the line's register word, which this crate carries into a
record and never reads. Chip facts select geometry and MNLBITS. Chip crates
and firmware use this crate directly.

Each image's static table of its interrupt bindings, declared once per image
with `interrupt_table!` (a chip's platform wraps it with its CPU lines, such
as `oer_chips_family_platform_runtime::interrupt_table!`), and the router that
writes the routes and lines by it.

One line, one part, one direct call. Each entry binds one interrupt part
to one CPU line at a level on a hart:

```text
contexts { app_wake: 1; radio_executor: 2; radio: 3; }
field: Token = Part => handler, line LINE, context radio, hart HART, CLASS;
```

Contexts name levels once per image. Every entry selects a declared context;
an unknown name fails to compile. The level check enforces the chip facts'
range for every declared context, including unused ones. Entries at each
level on each hart appear from lowest to highest arbitration priority. The macro checks that order against the
facts' tie-break: on S31 a later, higher-priority peer must have a higher line
ID. Peers at one level never preempt each other.

The sources come from the part's type (`oer_portable_mmio::InterruptSources`: the one
source of an `oer_portable_mmio::InterruptSource`, which the PAC generator implements
for a block's interrupt side, or the several sources of a part one handler
serves together), never from the entry's text, and the handler is a
`fn(&mut Part)` or, for an `Executor` entry, a type implementing
`ExecutorHandler<Part>` (the executor's `Round<Tasks>`), which the entry's
token states (`ExecutorEntry::Handler`) so that the executor's start can
check the task set it binds at compile time; it learns which source fired from its peripherals' status,
never from the matrix. A part made of several members builds its sources
with `interrupt_sources![Member, ...]`, at compile time; a source listed
twice fails to compile. For each entry the macro defines:

- `__oer_interrupt_h<HART>_l<LINE>`, the strong symbol the line's vector
  calls: through its one hidden entry point, `__private::vector`, it lends
  the bound part and the witness of the call (`Here<'_, Token>`) to the
  handler and runs nothing before the part is bound, never panicking. An
  entry that asks for the witness names it after its handler
  (`=> on_wifi, with here, line ...`), whose handler is then a
  `fn(&mut Part, Here<'_, Token>)`; root and every local Clippy policy
  forbid every other call of the entry point (`disallowed-methods`), and an
  entry's handler cell is the crate's own;
- a token type, neither `Copy` nor `Clone`, which `Interrupts::take` hands out
  once per image; `Entry` and the private token constructor are safe. The
  mandatory `EntryProof<Self>` binds its vector handler to the cell that
  handler reads. Root Clippy rules forbid direct `Unique::new` and
  `EntryProof::new` calls; only the macro's constructor statements allow them.
  Another entry's proof does not type-check. This is a lint policy: an
  explicit waiver can still supply false scheduling metadata to compile-time
  context checks. The macro also declares the table's type,
  `InterruptTable`, and makes it every entry's `Entry::Table`: an entry, its
  table element, its class and its handler and cell come from one
  declaration, so `Router::bind` asks nothing of the table at run time.
  Claimed storage yields `StorageClaimed` in quarantine;
- for an `Exempt(VIOLATION, "reason")` entry, the reason's bytes exported as
  `__OER_INTERRUPT_EXEMPT_` followed by the handler's symbol, for the image
  gate's report;
- with `#[cfg(...)]` before its doc comment, nothing at all in a build that
  leaves the entry out: its table element has no handler (a zero word).

The table holds one element per source of each entry, each with the
entry's line, level, hart, class and handler.

A level between the handlers and the application runs one task: an entry
of class `LevelTask` whose part lists no source, with the token, proof,
cell, storage and `Router::bind` of every entry, and a handler
`fn(&mut Part)`:

```text
keeper: KeeperToken = KeeperPart => on_keeper, line 34, context keeper, hart 0, LevelTask;
```

Its line is a software line its raisers pend. The image exports it in its
own table, `LEVEL_TASK_TABLE` (`__OER_LEVEL_TASK_TABLE`): line at 0, level
at 1, hart at 2 (`u8` each), the handler's word at 4, 8 bytes on the chip
(`TaskBinding`), with no source and no class; it has no element in the
source table. The table fails to compile when a `LevelTask` entry's part
lists a source or another entry's part lists none, two tasks share a level
and hart, or an entry of another class (an `Executor` or one with a
source) sits on the level of its hart's task. Hardware reaches a level's
task only through a handler above it, which pends it: a timer a task one
day needs is a `Hardware` entry that pends it, which is why a task's part
never has a source.

A wake from the same hart is the CLIC's pend of the woken line.
`Here<'_, E>`, zero-sized, neither `Send` nor `Sync` and bounded by the
call, witnesses that the code runs as entry `E`'s handler on `E`'s hart:
`Here::on_hart::<H>()` gives the proof of the hart
(`oer_portable_riscv::OnHart<H>`), the build checking `E::HART == H`. A
level task `T`'s pend, `Pend<T, R, X, L, H>`, names its two raisers in its
type: the handler of entry `R`, whose raise its witness proves
(`Pend::from_handler(&Here<'_, R>)`), and a task of the executor whose
entry is `X`, which raises with its hart's proof
(`Pend::from_task(&OnHart<H>)`, from the runtime's `Hart<H>::on_hart`).
`Pend::new` takes `T`'s `Bound` proof and the chip's handle of `T`'s line
(`PendLine<H>`, its line fixed at build), and the build checks that `T` is
a level task of hart `H` on that line, `R` strictly above it and `X`
strictly below it, all on hart `H`. A raiser holds the pend, never the
pending bytes with an index: nothing in a raiser can pend another line.

The table fails to compile when an entry's line, level or hart is outside
the chip's `Lines`, when two entries share a line and hart, when two entries
or one part list a source twice, when
two `Executor` entries share a level and hart, a `Hardware` or
`Exempt(WakerCall, ..)` entry sits on an executor's level of its hart, or an
`Exempt(ExecutorLevel, ..)` entry does not. A handler that takes another
part than its entry's, a token bound with a foreign part, a part bound twice
and a `Bound` made outside a binding fail too (the crate's compile-fail
tests). Each entry ends with its `Class`: `Hardware` (capture, acknowledge,
latch, raise a signal; strict code), `Executor` (an executor's interrupt:
strict up to its task polls) or `Exempt(VIOLATION, "reason")`: a hardware
handler that breaks the one named rule, `ExecutorLevel` (it sits on its
hart's executor level) or `WakerCall` (it calls a waker); the image gate
checks each class's code and waives only the named rule.

The table is `INTERRUPT_TABLE`, a slice of `#[repr(C)]` `Binding`s exported
as `__OER_INTERRUPT_TABLE` for the tools that read the image: source `u16` at
0, line at 2, level at 3, class at 4, hart at 5 (`u8` each), the handler's
word at 8, 12 bytes on the chip.
The table, handler and exemption symbols are exported through
`oer-portable-memory::link_fn!`, which owns the export attributes. This crate's unsafe
`LinkSlot` declarations reserve the table slice, vector function and exemption
byte-array types and names. Safe exports must match the declared slot's
name, kind and complete type, including function ABI. Each handler is a
safe associated C ABI function of its token, so the table uses its address
directly without a separate foreign declaration.

`Router` is the one writer of a hart's routes and lines, over the chip's
`Controller` (the hart's interrupt-matrix bank and its own CLIC; each chip
HAL's `platform::interrupt_controller` implements it on the typed blocks and
gives the CLIC three level bits). `install` takes the controller by value
and a hart's controller is made only on that hart from its one grant (the
staged runtime's handoffs), so each hart has one router, which stays on it.
Every bank is silenced before its hart first unmasks (the secondary launch
disconnects the released hart's bank; each router silences its own at
`install`) and each router routes only its hart's entries, so a source no
entry binds is routed on no bank:

- `Router<C, H, T>` names its hart and its table in its type (`T` is the
  image's `InterruptTable`, or `NoBindings`, the table of no entry). A table
  whose entry names a source, a line or a level the controller has not fails
  to compile. `Router::install` refuses a controller of another hart than `H`
  (`Error::WrongHart`) before any write, checks the controller's
  implemented geometry and level bits against its chip facts
  (`Controller::verify_configuration`), silences every source and disables
  every line of the hart, then verifies;
- `Router::bind` consumes an entry's token and its part, moves the part where
  only the entry's handler reaches it, binds the entry's line at its level
  (`Controller::bind` with its control byte, `oer_portable_riscv::clic::Control`:
  enabled, hardware-vectored, with the trigger the chip's bound lines have)
  and only then routes each of the part's
  sources to it (a `LevelTask` entry's line is bound by `Controller::bind_task`,
  positive-edge with its pending bit cleared before its enable, and nothing
  is routed to it), and returns
  `Bound<Entry>`, carrying `LEVEL`, `HART` and `CONTEXT` from that entry, the proof a driver takes before it enables the peripheral's
  interrupt. An entry of another table or another hart than the router's
  fails to compile; what `bind` can still answer is its storage claimed or
  its handler cell occupied, with its owners in `Quarantined`;
- `Router::silent` returns `Silent<Part>`, the proof that each of the part's
  sources is routed nowhere (read from the hardware) and no entry of the
  table binds it (at compile time), which a driver that
  polls the part's events takes instead of `Bound` (both implement the
  sealed `EventAuthority`);
- a controller's sources and lines are its own values (`Controller::Source`
  and `Line`), one for each it has in its constants `SOURCES` and `LINES`,
  which the router takes by an entry's number at compile time: a write names
  only what the controller has and returns nothing. The controller writes a
  line's enable apart from its attributes and control, so a line is never
  enabled with another level or trigger than its binding's;
- `Router::verify` checks the controller's configuration again, then that
  every routed source is an entry of the hart
  on its line, every routed line holds its entry's level, every level task's
  line of the hart is disabled or holds its task's binding
  (`LineExpectation::Task`, its control byte made at compile time from the
  table), and every other line is disabled.

A faulted binding records four words (`BindFault::diagnostic`): the binding
(the line in bits 0..8, the hart in 8..16; a line on a hart has one entry,
so they name it, a level task's as any other), then three zero words; the
reason names the fault. A wrong line `verify` finds is
reported with the line's register word as the chip read it
(`LineObservation`) and what the table expected of it
(`LineExpectation::record`: 0 disabled, bit 8 and the control byte bound,
bit 9 and the control byte a level task's).

An interrupt handler reaches its task only through a static signal
(`signal`), never through the router or a lock. A signal binds its executor's
entry and all raising contexts at construction:

```text
IrqSignal::<AppExecutor>::new::<
    (FromHandler<WifiToken>, FromOwnTask<AppExecutor, WakeToken>),
    WakeToken,
>()
```

`FromHandler<Entry>` must run above the receiving executor; `FromTask<X, Entry>`
must be a lower executor's task on its hart. `FromOwnTask<X, Entry>` is a
task of the receiving executor itself: it cannot preempt the executor's
round, so, as for an `Exempt(ExecutorLevel)` handler, its raise publishes
facts and pends the next round, which services them. `FromPeerHart<X, Entry>` is a
task of executor `X` on another hart than the receiving executor (checked at
compile time); no level relation holds across harts. A cross-hart wake is a
raise of the peer executor's signal, never a waker call: the raiser
publishes facts and pends the receiver through its software line, whose own
handler alone lowers the line and services them. `FromExecutorLevel<Entry>` admits only a
handler of class `Exempt(ExecutorLevel)` on the receiving executor's hart and
level. This handler cannot preempt the executor's round; it publishes atomic
facts and pends the executor, whose next round services those facts and wakes
the task in its own context. The handler calls no task waker and shares no
lock or task-owned signal state. Ordinary `FromHandler` still requires a
strictly higher level, including for exempt handlers.

Each executor entry must name its actual software interrupt source, all
participating contexts but a `FromPeerHart` raiser share a hart, and an
interrupt-mode executor's entry has class `Executor`. A thread-mode executor's
wake entry represents its tasks at level zero. Nested pairs declare several
raisers of one signal; the signal's layout and atomic publication are unchanged.
The composition must keep `raise` calls within those declared contexts;
`raise` itself does not check the caller. A raise publishes its facts
(fetch-or, Release) before the executor's pend, whose software-line raise
fences `w, o`; the receiving handler lowers its line and fences `o, rw`
before it swaps the facts (`oer-chips-family-platform-executor`'s software line, which
states the hardware preconditions of the cross-hart order).

`Doorbell::new`, Wi-Fi request activation and BLE gate exchange assert the
same request-direction predicate: the handler must be above the requesting
executor on the same hart. Wi-Fi and BLE also check that the doorbell names
the bound handler. There is no separate three-level or same-level proof.
The typed `Doorbell<SWI, HandlerEntry, ExecutorEntry>` retains both entries
and checks the requester's `Executor` class; image wrappers derive their
metadata from that type. Static signal initializers are checked by
`cargo check`. The generic runtime constructors and driver calls evaluate
their inline const checks at monomorphization, covered by paired codegen
tests here and in the executor and driver crates. Stable Rust cannot use these
associated scheduling constants in generic const expressions in a signature.

```console
cargo test -p oer-chips-family-platform-interrupt-table
```

Signal memory admission belongs to `oer_portable_memory::ExecutionContext`.
`SignalExecutor` is safe and names its context; `Entered<Context>` checks
both entry and every `ContextCell` access.
`oer_portable_memory::signal::{SignalCore, SignalList}` own the task cell, atomic facts
and intrusive links and carry that context type, so one context's token
cannot access another's state.

With `host-signal`, `host::signal()` and `host::deferred_signal()` return
`(owner, raiser)`. The owner is a local `&'static IrqSignal` that is !Send
and !Sync: only its creating thread takes, polls or services payload.
The `oer_portable_memory::signal::Raiser` is Send + Sync and contains only the atomic
pending-word reference. Its `raise` coalesces facts without servicing payload or invoking
a waker; the owner services them explicitly. Raising through the local
`Host` owner retains immediate local service; `Deferred` waits for a take
or service. No transferable handle exposes the owner or context proof.
