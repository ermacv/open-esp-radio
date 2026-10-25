# oer-interrupt-table

Each image's static table of its interrupt bindings, declared once per image
with `interrupt_table!` (a chip's platform wraps it with its CPU lines, such
as `oer_espressif_staged_runtime::interrupt_table!`), and the router that
writes the routes and lines by it.

One line, one source, one direct call. Each entry binds one interrupt part
to one CPU line at a level on a hart:

```text
field: Token = Part => handler, line LINE, level LEVEL, hart HART, CLASS;
```

The source comes from the part's type (`oer_mmio::InterruptSource`, which
the PAC generator implements for a block's interrupt side), never from the
entry's text, and the handler is a `fn(&mut Part)`. For each entry the macro
defines:

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

The table fails to compile when an entry's line, level or hart is outside
the chip's `Lines`, when two entries share a line and hart or a source, when
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

- `Router::install` silences every source and disables every line of the
  hart, then verifies;
- `Router::bind` consumes an entry's token and its part, moves the part where
  only the entry's handler reaches it, writes the entry's line (enabled,
  hardware-vectored, positive-level-triggered, its level in the control
  byte, `line_control`) and only then routes the source to it, and returns
  `Bound<Part>`, the proof a driver takes before it enables the peripheral's
  interrupt; an entry of another hart or one the table lacks comes back with
  its token and part;
- `Router::verify` checks that every routed source is an entry of the hart
  on its line, every routed line holds its entry's level, and every other
  line is disabled.

An interrupt handler reaches its task only through a static signal
(`signal`), never through the router or a lock.

```console
cargo test -p oer-interrupt-table
```
