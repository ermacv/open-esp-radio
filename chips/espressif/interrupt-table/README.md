# oer-espressif-interrupt-table

Espressif interrupt-matrix bindings and vector ABI, including the symbols
consumed by the staged runtime. Architectural CLIC line values come directly
from `oer_riscv::clic`; chip facts select geometry and MNLBITS. Chip crates
and firmware use this crate directly.

Each image's static table of its interrupt bindings, declared once per image
with `interrupt_table!` (a chip's platform wraps it with its CPU lines, such
as `oer_espressif_staged_runtime::interrupt_table!`), and the router that
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

The sources come from the part's type (`oer_mmio::InterruptSources`: the one
source of an `oer_mmio::InterruptSource`, which the PAC generator implements
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
  calls: it lends the bound part to the handler and runs nothing before the
  part is bound, never panicking;
- a token type, neither `Copy` nor `Clone`, which `Interrupts::take` hands out
  once per image; `Entry` and the private token constructor are safe. The
  mandatory `EntryProof<Self>` binds its vector handler to the cell that
  handler reads. Root Clippy rules forbid direct `Unique::new` and
  `EntryProof::new` calls; only the macro's constructor statements allow them.
  Another entry's proof does not type-check. This is a lint policy: an
  explicit waiver can still supply false scheduling metadata to compile-time
  context checks. `Router::bind` checks source, line, level, hart, class and
  handler/cell identity before placement or controller writes. Mismatches
  return typed refusals; claimed storage yields `StorageClaimed` in quarantine;
- for an `Exempt(VIOLATION, "reason")` entry, the reason's bytes exported as
  `__OER_INTERRUPT_EXEMPT_` followed by the handler's symbol, for the image
  gate's report;
- with `#[cfg(...)]` before its doc comment, nothing at all in a build that
  leaves the entry out: its table element has no handler (a zero word).

The table holds one element per source of each entry, each with the
entry's line, level, hart, class and handler.

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
`oer-memory::link_fn!`, which owns the export attributes. This crate's unsafe
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

- `Router<C, H>` names its hart in its type. `Router::install` refuses a
  controller of another hart than `H` (`Error::WrongHart`) before any
  write, checks the controller's implemented geometry and level
  bits against its chip facts (`Controller::verify_configuration`), refuses
  an entry of the hart whose source, line or level lies outside the
  controller (`Error::NoSuchSource`, `NoSuchLine`, `NoSuchLevel`) before any
  write, silences every source and disables every line of the hart, then
  verifies;
- `Router::bind` consumes an entry's token and its part, moves the part where
  only the entry's handler reaches it, writes the entry's line (enabled,
  hardware-vectored, positive-level-triggered, its level in the control
  byte, `oer_riscv::clic::Control`) and only then routes each of the part's
  sources to it, and returns
  `Bound<Entry>`, carrying `LEVEL`, `HART` and `CONTEXT` from that entry, the proof a driver takes before it enables the peripheral's
  interrupt; an entry the table lacks comes back with its token and part,
  and an entry of another hart than the router's or outside the controller
  fails to compile;
- `Router::silent` returns `Silent<Part>`, the proof that each of the part's
  sources is routed nowhere and no entry of the table binds it, which a driver that
  polls the part's events takes instead of `Bound` (both implement the
  sealed `EventAuthority`);
- a controller refuses a source or line outside it and writes nothing; it
  writes a line's enable apart from its attributes and control, so a line is
  never enabled with another level or trigger than the router's;
- `Router::verify` checks the controller's configuration again, then that
  every routed source is an entry of the hart
  on its line, every routed line holds its entry's level, and every other
  line is disabled.

An interrupt handler reaches its task only through a static signal
(`signal`), never through the router or a lock. A signal binds its executor's
entry and all raising contexts at construction:

```text
IrqSignal::<RadioExecutor>::new::<
    (FromHandler<WifiToken>, FromTask<AppExecutor, WakeToken>),
    RadioToken,
>()
```

`FromHandler<Entry>` must run above the receiving executor; `FromTask<X, Entry>`
must be a lower executor's task on its hart. `FromPeerHart<X, Entry>` is a
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
before it swaps the facts (`oer-espressif-executor`'s software line, which
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
cargo test -p oer-espressif-interrupt-table
```

Signal memory admission belongs to `oer_memory::ExecutionContext`.
`SignalExecutor` is safe and names its context; `Entered<Context>` checks
both entry and every `ContextCell` access.
`oer_memory::signal::{SignalCore, SignalList}` own the task cell, atomic facts
and intrusive links and carry that context type, so one context's token
cannot access another's state.

With `host-signal`, `host::signal()` and `host::deferred_signal()` return
`(owner, raiser)`. The owner is a local `&'static IrqSignal` that is !Send
and !Sync: only its creating thread takes, polls or services payload.
The `oer_memory::signal::Raiser` is Send + Sync and contains only the atomic
pending-word reference. Its `raise` coalesces facts without servicing payload or invoking
a waker; the owner services them explicitly. Raising through the local
`Host` owner retains immediate local service; `Deferred` waits for a take
or service. No transferable handle exposes the owner or context proof.
