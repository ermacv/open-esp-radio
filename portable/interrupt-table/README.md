# oer-interrupt-table

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
`fn(&mut Part)`; it learns which source fired from its peripherals' status,
never from the matrix. A part made of several members builds its sources
with `interrupt_sources![Member, ...]`, at compile time; a source listed
twice fails to compile. For each entry the macro defines:

- `__oer_interrupt_h<HART>_l<LINE>`, the strong symbol the line's vector
  calls: it lends the bound part to the handler and runs nothing before the
  part is bound, never panicking;
- a token type, neither `Copy` nor `Clone`, which `Interrupts::take` hands out
  once per image; `Entry` is an `unsafe` trait and the token's constructor an
  `unsafe fn`: only the macro creates a token;
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

`Router` is the one writer of a hart's routes and lines, over the chip's
`Controller` (the hart's interrupt-matrix bank and CLIC; each chip HAL's
`platform::interrupt_controller` implements it on the typed blocks and
gives the CLIC three level bits):

- `Router::install` checks the controller's implemented geometry and level
  bits against its chip facts (`Controller::verify_configuration`), refuses
  an entry of the hart whose source, line or level lies outside the
  controller (`Error::NoSuchSource`, `NoSuchLine`, `NoSuchLevel`) before any
  write, silences every source and disables every line of the hart, then
  verifies;
- `Router::bind` consumes an entry's token and its part, moves the part where
  only the entry's handler reaches it, writes the entry's line (enabled,
  hardware-vectored, positive-level-triggered, its level in the control
  byte, `line_control`) and only then routes each of the part's sources to
  it, and returns
  `Bound<Entry>`, carrying `LEVEL`, `HART` and `CONTEXT` from that entry, the proof a driver takes before it enables the peripheral's
  interrupt; an entry of another hart or one the table lacks comes back with
  its token and part, and an entry outside the controller fails to compile;
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
must be a lower executor's task. `FromExecutorLevel<Entry>` admits only a
handler of class `Exempt(ExecutorLevel)` on the receiving executor's hart and
level. This handler cannot preempt the executor's round; it publishes atomic
facts and pends the executor, whose next round services those facts and wakes
the task in its own context. The handler calls no task waker and shares no
lock or task-owned signal state. Ordinary `FromHandler` still requires a
strictly higher level, including for exempt handlers.

Each executor entry must name its actual software interrupt source, all
participating contexts share a hart, and an
interrupt-mode executor's entry has class `Executor`. A thread-mode executor's
wake entry represents its tasks at level zero. Nested pairs declare several
raisers of one signal; the signal's layout and atomic publication are unchanged.
The composition must keep `raise` calls within those declared contexts;
`raise` itself does not check the caller.

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
cargo test -p oer-interrupt-table
```
