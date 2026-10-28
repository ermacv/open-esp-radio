# Trace events and the observer contract

`oer-trace` is a portable `no_std` crate without `unsafe` code, shared by
the target and the host decoder. It holds typed events, their two-word
encoding and decoding, ordering of caller-owned record slices, and the
contract through which an owner reports observations. It has no global
recorder, clock, placement, persistence or boot policy; the platform's
recorder (`oer-espressif-staged-runtime::trace`) owns those.

## Events

An `Event` has a `Kind` (its `Domain` and the domain's event id), a `Channel`
(a host-controlled bit of the 64-bit mask, in a fixed range per domain) and
an encoding into two `u32` words that the host decodes with the same type.
`event_set!` and `Describer` render records by set; an unknown kind prints
generically. `Record` is the 16-byte entry; `oldest_first` orders a slice by
its cyclic tags.

## Observers

An owner (a core, runtime, driver or PHY owner) holds an `O: Observer`
field, by default `Noop`, and passes `&O` to its children. It reports with

```rust
oer_trace::observe!(self.observer, Step { state: self.state, at: clock.now() });
```

- `Observer::event` takes an event value that is `Copy` and `'static` and
  returns `()`. An affine owner (register block, lease, permit) or a borrow
  of one cannot be an observation, and an observation cannot steer the
  owner. The method is generic, so there is no `dyn Observer`: every call is
  direct and visible to stack analysis.
- `Noop` keeps nothing. A const item checks that it has size 0, alignment 1
  and no drop glue, so an owner's `Noop` field costs no memory or code.
- `&O` is an observer that forwards to `O`.
- `observe!` is the only observation point. Its observer operand is a
  binding or a field path, never a call. With `cfg(oer_trace)` it passes the
  event to the observer. Without it the event expression is removed before
  type checking: nothing in it is evaluated (no clock, register or frame
  read), the observer is only borrowed, and the point leaves no code. A value
  computed only for an observation is computed inside the point.

`cfg(oer_trace)` is the image's switch. `cargo fw` sets it only for an image
that declares the `trace` instrumentation class. Instrumentation is never a
Cargo feature of a library; `oer-test-layers` checks feature names.

## Checks

- Const item: `Noop` size, alignment and drop glue.
- Unit tests (`src/tests.rs`): without the cfg a point evaluates no argument
  and reports nothing; with `--cfg oer_trace` it evaluates its event once and
  reports it; observers by reference forward.
- `tests/codegen.rs`: an owner with points, built without the cfg for
  `riscv32imafc-unknown-none-elf` with the firmware profile's code
  generation, emits the same assembly as the owner without them; with the
  cfg the points add the clock calls.
