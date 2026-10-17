# oer-interrupt-table

Each image's static table of its peripheral interrupt sources: for every
source its handler, its level and its core, declared once per image with
`interrupt_table!` (a chip's platform wraps it with its types, such as
`oer_espressif_staged_runtime::interrupt_table!`).

The table says where a source may be routed; the source's owner decides when.
For each entry the macro defines:

- the source's strong handler symbol, named as the source, which the chip's
  vector slot of the source links to: a second handler for one source fails
  the link;
- a token type, neither `Copy` nor `Clone`, which `Interrupts::take` hands out
  once per image. `enable` and `disable` take only a token; `enable` routes
  the source to the table's level on the table's core and fails on another
  core, or for a token whose source the table does not list so. `Entry` is an
  `unsafe` trait and the token's constructor an `unsafe fn`: only the macro
  creates a token;
- with `#[cfg(...)]` before its doc comment, nothing at all in a build that
  leaves the entry out: its table element has no handler (a zero word).

The table is `INTERRUPT_TABLE`, a slice of `#[repr(C)]` bindings exported as
`__OER_INTERRUPT_TABLE` for the tools that read the image; the image hands it
to its platform's runtime as a typed value.

A `Route` is a token with its type erased: only consuming a token creates
one, it is neither `Copy` nor `Clone`, and `enable_route`/`disable_route`
check it as `enable`/`disable` check the token; an owner keeps its sources'
routes behind a concrete type. `Adopted` holds the image's table for every
core: adopted once (a second adoption is an error), absent before.

`install` silences every source of the current core's entries and checks each
vector slot against the table; `verify` checks again that every slot holds its
handler and every source is silent or routed to its level; `verify_required`
checks that each source a driver waits on has an entry and, on its entry's
core, is routed. `Matrix` is the
chip's interrupt matrix as the table drives it; `__fake_matrix!` is a host
model for tests.

On the ESP32-S31 the table is the only owner of routes: every esp-hal build
for the chip enables the fork's `static-interrupts`, under
which esp-hal has no API that binds a handler or maps a source at run time.
The platform's table adopts esp-hal's one routing capability; an esp-hal
driver that needs its interrupt (`into_async`) panics unless the image's
table holds the source's handler and its owner has already routed it, and
records the source for `verify_required` before interrupts are enabled. An
owner therefore routes its source with its token before it hands the driver
its interrupt.

```console
cargo test -p oer-interrupt-table
```
