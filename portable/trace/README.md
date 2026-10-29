# Observation values and counters

`oer-trace` is a portable `no_std` crate without `unsafe` code. It holds
small event values, their dictionary encoding and the observer contract.
Observation has one mechanism: owners report values through `observe!` and
an image-selected collector increments atomic counters by event kind.
There is no observation ring, frame snapshot, retained history or trace clock.

An owner holds an `O: Observer` field, by default the zero-sized `Noop`, and
passes its observer by reference to children:

```rust
oer_trace::observe!(self.observer, Step { state: self.state });
```

`Observer::event` takes a `Copy + 'static` event value and returns `()`.
It holds no owner, register block, lease or frame borrow and cannot steer
an operation. Dispatch is generic and direct, visible to stack analysis.
Owners hold no observation counters, event buffers or copied frame data.
The image supplies the collector; product compositions select `Noop`.

`observe!` is the only observation point. Its observer operand is a binding
or field path. Without `cfg(oer_trace)` the event expression is removed before
type checking: no clock, register, frame read or collector update is evaluated.
The observer is only borrowed and the point leaves no code. Values needed
only for observation are computed inside the point.

An `Event` has a `Kind` and a dictionary `Channel`, and an encoding of two
`u32` words for typed image adapters. Collectors count kinds without encoding
or keeping event payloads. Channels identify dictionary groups; no recorder
mask or history filtering exists. Protocol and chip vocabularies are host-safe.

`cfg(oer_trace)` is the image's switch. `cargo fw` sets it for images declaring
the `trace` class; libraries expose no instrumentation Cargo features.
`oer_debug_control` separately controls test holds and injections. Fault records
in `oer-diagnostics` and retained fault owners are failure contracts, not
observations; their lifecycle is independent.

Unit tests check `Noop` geometry, reference forwarding and argument evaluation.
`tests/codegen.rs` checks equal RISC-V assembly for an owner with disabled
points and the same owner without points. Image collector tests check atomic
kind counts, supported zeros, saturation, concurrent updates and disabled points.
