# Radio permit pool

`oer-radio-permit-pool` implements the `PermitPool` of
[`oer-radio-port`](../port/README.md) without allocation:
`FifoPermitPool<'id, K, PERMITS, SEATS, Pool, Waiters>` consumes the
indivisible `PermitSupply` from one component of the installation layout, and serves its takers first in,
first out. Its line holds a place for each of the `SEATS` seats `Permits`
made with the `Line<'id, K, SEATS, Waiters>` the pool is built with, and each taker
waits with its own seat. A backend keeps it in its held part of the install
(`PortBackend::Held`), so the permits of submitted work come back to it before
their terminal events are handed out.

A line with any seats requires nonzero permit capacity, checked when its
constructor is instantiated. Zero permits and zero seats remain a valid empty
component. A zero-capacity line with live waiters cannot promise progress from
any terminal, so it cannot be built.

- A taker takes its seat's place in the line at its first poll and is served
  once it leads and a permit is free. A taker whose future drops gives up its
  place, and its seat stays with it.
- A released permit wakes the leader. A served taker that leaves a permit free
  wakes the next leader.
- Each permit returns to its own numbered place. The indivisible supply and
  its sealed component path prove both the index bound and that no other
  outstanding permit occupies that place; returning capacity needs no search
  for an empty slot.
- The line has a place for every seat, so it is never full and arrival order
  holds for every taker. A `Line` is a copyable receipt: several pools whose
  takers are the same seats (one per transmit queue) each take it. Their permit paths stay distinct while the shared waiter path identifies those seats. A
  take that leaked without its drop (`mem::forget`) loses its place to its
  seat's next take.
- The pool is `!Sync`. It lives in the port body's task, and nothing outside
  that task reaches it.

Its tests run the shared rule `conformance::permits_serve_takers_in_arrival_order`
on a pool of one permit and on a pool with a permit held aside, and check
arrival order across every seat, a dropped and a leaked take, and the wakes of
the leader and the next leader.
