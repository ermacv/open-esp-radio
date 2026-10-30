# Event sinks

`oer-events` is a portable `no_std` crate without `unsafe` code or
dependencies. It holds the contract through which an owner reports what it
decided to the caller that lent it a sink for one call.

```rust
pub fn receive(&mut self, frame: Frame, events: &mut impl EventSink<Event>) -> Disposition {
    // ...
    oer_events::report(events, || Event::Dropped { reason });
}
```

- The owner takes `&mut impl EventSink<E>` as a call parameter and never
  stores it. It passes the same loan to its children.
- An event is a `Copy + 'static` value of the owner's own type: a variant or
  a struct with the fields the decision already computed. It carries no
  global identifier, encoding or time, and no borrow of frame bytes.
- `EventSink::event` returns `()`: a sink cannot steer the owner.
- `report` builds the event only when `EventSink::ENABLED` is `true`. `()`
  is the sink that keeps nothing; with it no event argument is evaluated
  and the report point leaves no code.
- What an event becomes, which time it is bound to and how another context
  reads the result belong to the caller that chose the sink.

Unit tests (`src/tests.rs`) check that `()` builds no event, that an
enabled sink receives each event once and that a lent loan forwards to the
caller's sink.
