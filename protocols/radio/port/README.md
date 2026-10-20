# Radio port lifecycle and vocabulary

`oer-radio-port` owns the lifecycle shared by the three radio blocks'
ports (Bluetooth LE, IEEE 802.15.4 and the IEEE 802.11 lower-MAC port) and
their capacity, error and clock vocabulary. The first two lifecycle
implementations had identical product code; each also exposed the same
failing cases: a retirement waiter received no wake, and an untyped driver
exit was accepted as a successful uninstall. Their common contract lives
here.

| Value | Meaning |
| --- | --- |
| `Installed`, `InstallParts`, `PortBackend`, `PortBody`, `Owner`, `Ended`, `DriverLoan`, `DriverExit`, `Retired`, `Detached`, `Quarantined` | One installed backend and independent driver, a borrowing typestate owner, and affine proofs of driver completion and detachment |
| `branded`, `Mint`, `PoolSet`, `PoolPath`, `PermitSupply`, `Slots`, `Permits`, `Slot`, `Permit`, `PermitPool`, `Seat`, `Line` | Capacity as branded tokens. `branded` runs an install with the `Mint` of a fresh brand `'id`, generic over it, so nothing branded leaves the install or reaches another; `Mint::into_pools(self)` consumes its authority and issues the complete layout declared by the backend's `Pools` associated type. `Slots<K, N>` supplies synchronous slots; `Permits<K, N, S>` supplies awaited permits, their line and caller seats. Custom layouts consume one `PoolBuilder<'id, Path>` to expand their components. Each tuple derives a sealed `Child<Path, INDEX>` for each member; every slot, permit, seat and line retains that identity, including repeated kinds in nested layouts. A FIFO consumes the indivisible `PermitSupply` once, so subsets cannot form interchangeable FIFOs. Neither authority can issue a second set; no token is `Clone`, `Copy` or `Default` |
| `PortResult`, `Poisoned` | Commands and clock calls return `Result<Result<T, Refusal>, Poisoned<Fault>>`. The inner `Err` is a typed refusal (nothing changed); the outer one carries the backend's cause, which portable code passes on without interpreting it |
| `EventEnd<Fault>` | `next_event` alone returns `Result<Result<Event, EventsLost>, EventEnd<Fault>>`. `Drained { cause }` promises that every terminal of admitted work was delivered. A missing terminal violates the contract and can be detected by the body. `Quarantined { cause }` leaves undelivered owners in the `Quarantined` of `PortEnd`. The end contains only its cause, no owners or affine proofs, and every subsequent call returns the same variant and cause |
| `EventsLost` | Events were dropped. Reported once, in place of the first dropped event; events before it precede the gap and events after it follow it. A backend reserves at admission the slot of every terminal event and of data its protocol promises to deliver (acknowledged Bluetooth LE connection data), so the gap holds only data the protocol does not promise (received frames, advertising reports) and the consumer continues |
| `ClockInfo<D>`, `ClockRelation<A, B>`, `RelationRate`, `RelationSample<A, B>`, `Generation<A, B>`, `GenerationSource<A, B>`, `Stamp<A, B>`, `Projected<T, S>`, `ProjectionError` | One relation between two clock domains, typed by its source `A` and target `B`: identical, or affine within a drift bound. A `RelationSample` pairs one reading of each clock with its uncertainty (a target span) and its generation; `project`, `project_back` and `project_span` are the one projection both ways, returning the value and an uncertainty that adds the drift over the distance from the sample, as a span of the target domain. A coordinate of another generation is `StaleSample`, a result outside the range `OutOfRange`. A generation has no public constructor: the relation owner's `GenerationSource` opens an epoch and jumps at each break. `ClockInfo<D>` states a port's resolution and its relation to monotonic time (`monotonic`, `None` when none exists); `RadioStamp<D>` and `ClockSample<D>` are the radio-to-monotonic `Stamp` and `RelationSample` |

## Conformance

The `conformance` module (the `test-support` feature) holds the rule every
permit pool meets, `permits_serve_takers_in_arrival_order`: its pool's
owner (a backend's held part, a runtime) calls it with the pool, three
seats of its line and at most eight permits it holds aside.

## Component authority and cost

`Root` is the path of a standalone layout. A tuple adds one `Child` per
component at each nesting level; custom factories preserve the path they
receive. The default `Root` type parameter describes a standalone pool,
not a way to erase a tuple component's identity. A line is a copyable receipt
for its waiter domain. A FIFO's capacity comes from its consuming supply;
several supplies can deliberately use one line while their permit paths
remain distinct.

| Consumer | Authority retained | Runtime cost of the path | Construction and cancellation |
| --- | --- | --- | --- |
| Wi-Fi | Gate and issuer, TX and Block Ack tickets, buffer/key pools, station connection source and its contexts | Zero additional bytes; indices remain `u8`, the epoch remains a `u64` | The phase accepts its associated gate path; terminals retain the submitted slots and connection context |
| Bluetooth LE | The backend's declared layout; the port test backend has a standalone slot component, production layouts without pools use `()` | Empty layout and Mint remain zero sized | Existing branded install/body lifecycle; consuming issuance has no new waits or cancellation state |
| IEEE 802.15.4 | Nested interface/configuration/pending-table paths and the separate radio-operation path | Zero additional bytes; each slot/permit/seat remains one byte | Configured and pending-entry owners retain their slot; operation guards release only to their source pool |
| FIFO runtime | Independent permit and waiter paths; opaque initial supply | No registry, allocation or new synchronization; line receipt remains zero sized | The whole supply constructs one pool; cancelled takes release their queue place and keep the borrowed seat |

Contract misuses and their positive twins live in `tests/ui/` and the
consumers' corresponding directories; each misuse must fail to compile on
host and RV32. Unit tests cover shared waiter lines,
cancellation, empty layouts and all 256 indices.

## Lifecycle and cancellation

`run_port(installed, body, execution)` selects driver execution explicitly.
The lifecycle gives `PortExecution::start` one `PortDriver`, which retains
the single driver loan, held-state borrow and retirement receiver. The
returned future joins the driver's proved exit. The body stays in its
caller's executor context. `LocalExecution` polls the driver beside it and
supports non-`Send` host models. It establishes no synchronization between
contexts: the S31 conformance image currently selects this local execution
on P2 and does not exercise the P2 driver / P1 role-body boundary specified
in [the architecture](../../../docs/architecture.md#execution-contexts).

An execution adapter must end every possible poll before releasing the
driver's scoped borrow, including on cancellation. An ordinary static spawn
does not satisfy this rule. The backend separately establishes its queue
ownership and any bounds needed for transfer between contexts.

The current borrowed `PortDriver` does not provide permanent backing for
deferred destruction on another executor. Cancelling `run_port` ends its
loan of the driver's owner and `Held`; `Quarantined` suppresses destructors
but cannot extend those borrows or preserve the containing future's storage.
Before the first poll, `PortExecution::start` has not run at all. The
current cancellation guard calls the backend directly, so this API alone
does not implement the driver-context abandonment rule in
[the decisions](../../../docs/decisions.md#radio-ports). The
`driver_task_static_escape` UI pair rejects retaining the borrowed task for
later destruction and permits a join that ends within its original scope.

`Installed::new(backend, driver)` is the sole constructor. `run_port` consumes
it, keeps `backend` by value, and lends `&mut B` to `Owner<'p, 'id, B, State>`.
The core keeps the driver by value beside the backend. Static
`PortBackend::drive` consumes its single affine `DriverLoan`; that loan
exposes shared driver operations, with no extraction or whole-driver
replacement. Retirement consumes that loan through `Retire::requested`;
`DriverLoan::fail_stop` consumes the loan and `PublishedPoison` to issue the
fault exit proof. Both handoffs end driver access by type. Its signature
has no backend receiver to borrow. The driver
has no installation lifetime: branded queues,
`Slot<'id>` and in-flight ownership belong in `Held<'id>`. The named
`InstallParts<'id, B>` separates held queues, body-local accounting and the
resources delivered to the body. The single mint still consumes the whole
capacity layout once.

An ordinary fault or `Abandon::Stopped` ends its event stream with `Drained`
after all admitted terminals are delivered. `Abandon::Unconfirmed` ends with
`Quarantined` immediately, before waiting or acquiring the ISR's mutex. The
body then fail-stops without further draining or ordinary disable; `PortEnd`
retains the owners. BLE and IEEE 802.15.4 keep the final end in their local
installation state so a new phase loan cannot reopen the stream.
An accounting violation also ends delivery with `Quarantined`, preserving
the real admitted owners. It does not decide the hardware outcome: the
subsequent abandonment can still return `Stopped`.

Enable and disable are consuming, bounded transitions. Both `enable` and
`setup` receive `&Held<'id>` and `&mut Local<'id>`, as do phase and disable;
setup's loan ends before enable can consume its owner. A running phase is
one invariant `Lent`; its borrowed `PhaseParts` can serve disjoint work in
the same task. Terminal capacity is reserved when work is admitted, and
disable hands all remaining terminals to its sink before success. The sink
returns `ControlFlow::Continue(())` when it accepts a terminal, or
`ControlFlow::Break(terminal)` to return it by value. A rejection ends delivery
immediately with `PoisonedOwner`; the backend retains that terminal and every
remaining owner. Uninstall returns the retained owners or keeps them in
`Quarantined`, without dropping them. The backend
states each transition bound. The retirement deadline future is created
before the owner loan and cannot borrow the backend (`use<Self>`); it starts
on its first poll after retirement is requested.

A driver returns `DriverExit::Retired` with a proof obtained by consuming
`Retire` and the driver loan after its request, or `DriverExit::Faulted`
with a proof obtained by publishing the port's `PoisonState` and consuming
the loan with that publication. The core retains a faulted driver's owners
while the body drains earlier events and requests uninstall. Uninstall takes
`Retired<'id, (Driver, State)>` and `Detached` of the same brand. The ended
drive future releases only its loan; ordinary uninstall receives the driver
by value with its completion state. There is no untyped early exit.

Retirement and abandonment register and wake their waiters. The abandon latch
has independent core and driver waiters. A dropped owner wakes the core,
which closes admission, requests retirement and detaches after the driver
ends; a missed deadline fail-stops. `Owner::fail_stop` calls `abandon`
synchronously and returns an affine `FailStopped<'id, B>` proof. `abandon`
poisons the installation without unbounded waiting. The core guarantees
at most one call: after abandonment it consumes the armed cancellation guard
and keeps a guard that only quarantines the owners. While a fail-stopped body
still holds its loan, cancellation checks the latched cause instead of
calling the backend again. Backends need no memoized stop outcome. The
latch preserves its first cause through a `LIVE` compare-exchange.
`abandon` returns `Abandon::Stopped` only with its `HardwareStopped` proof
of masked IRQ entries, bounded CPU exclusion and device stopping. `Abandon::Unconfirmed(Quarantine)`
consumes the authority without claiming those effects; it requires chip reset.
An overlong driver step or ISR may leave hardware running: every reachable
allocation stays in permanent quarantine until chip reset. Neither outcome
authorizes DMA backing reuse. `FailStopped::outcome` exposes this distinction
without consuming the body's affine proof.
The core retires independently while the body waits; only final `detach`
and result delivery wait for the body. Detach consumes its own affine call
authority and borrows the outcome, which remains available to the caller.
Cancellation of `run_port`, including before its first poll, applies the
same synchronous quarantine rule. There is no result receiver during
cancellation, so the core explicitly retains the outcome with the owners.

`Quarantined<S>` holds its state in `ManuallyDrop` and has no accessor or
extraction operation. Dropping a receipt runs no owner destructor, frees no
backing and touches no hardware. Both abnormal `PortEnd` variants retain
the backend, driver and `Retirement<State>`; a timed-out drive future ends
only its loan, while the driver remains owned by the core. A completed
faulted driver's state is likewise retained if the body is cancelled while
draining. Branded held/local values cannot leave the installation and are
retained without Drop on these paths. Only ordinary uninstall extracts them.

Forgetting an owner cannot construct `Ended`. Hardware-reachable memory must
have static storage outside the movable driver and its future. Neither a
loan nor `ManuallyDrop` extends a borrow or preserves inline storage after
its containing allocation ends. Backends must leave progress and hardware
owners in their driver or that durable backing at every `await`, in a
state that retire and abandon can handle. Cancellation tests poll the BLE
clock, stop, transition and retirement waits and the 802.15.4 idle/backoff/
retry waits, drop the drive future, then finish retirement and uninstall
with the same owners. Drop counters cover unconfirmed exits and receipt
destruction; forgetting the complete future preserves its durable backing.
No allocator or unsafe code is needed by this core.

The body chooses `PortBody::Output` before its generic `run` introduces `'id`;
that output cannot contain the installation's brand. It returns
`Ended::uninstall(owner, control, output)` or
`Ended::fail_stopped(proof, output)`. `run_port` returns `PortEnd::Uninstalled`
with owners and output. `FailStopped` carries output, `Abandon` and
`Quarantined`; `Abandoned` carries the latter two when a dropped owner made
the core end the installation without a body result. The caller can
inspect `Stopped` versus `Unconfirmed`; quarantine permits no backing reuse,
and an unconfirmed stop requires chip reset.
BLE's service returns `Served` through this output instead of a side channel.

## Cost

The RV32 measurement uses two concrete backends and the same enable,
disable, retire, detach and cancellation workload in
[the lifecycle cost probe](examples/lifecycle_cost.rs). With Rust 1.97.1,
`riscv32imafc-unknown-none-elf`, opt-level 3, one codegen unit, LTO and aborting
panics, the shared lifecycle measures 6844 bytes of `.text`; two separate
per-protocol lifecycles measure 2282 bytes (record outside this repository).
This is a small lifecycle ELF, not total firmware size or a hardware timing
result. It includes the cancellation guard before the first poll and the
retained outcomes and owners; the fixture has no backend stop memo whose
removal could offset this cost. The future is 432 bytes per backend. The
fallible disable sink accounts for 416 bytes of the text, mostly in the two
body state machines (630 bytes each), which handle the sink's returned
terminal and poisoning path; the `EventEnd` event end adds none. The fixture
does not poll its event stream, so this measures lifecycle cost, not
event-dispatch or total BLE firmware cost. Each backend also keeps
`Option<EventEnd<Fault>>` in `Local`; its size depends on the fault type.

| Checks | Code size | RAM and CPU | Signatures | Breakage | Alternatives | Reversal |
| --- | --- | --- | --- | --- | --- | --- |
| Typestate and opaque affine call/retire/poison/detach proofs are compile-time checks. Hardware refusal, poisoning and bounded waits remain runtime outcomes; lifecycle code has no explicit panic. | Test ELF `.text`: 6844 bytes. Backend/body futures remain concrete monomorphizations; retirement and signal machinery add code. | No allocation. The measured RV32 future is 432 bytes per backend. Three atomic waiter slots per installation: retirement, driver abandonment and core abandonment. Wake registration uses atomic synchronization; consuming proof and call tokens are zero-sized. Hot radio operations acquire no new lifecycle lock. | `Installed<B>`, `InstallParts<'id, B>`, static `drive(DriverLoan, held, retire)`, `DriverExit`, `Call`, `Quarantine`/`Abandon`/`HardwareStopped`/`Detach`, `Retired<'id, (Driver, State)>`, `Detached<'id>`, `Quarantined<S>`, `PortEnd<B, Out>`, `disable` sink returning `ControlFlow<Terminal>`, `EventEnd<Fault>` for `next_event`; owner borrows `&mut B`, `Ended<'id, B, Out>` has no borrow lifetime. | Both blocks consume `Installed`, borrow one phase, and retain terminals rejected by disable. Always-accepting sinks return `Continue(())`. The IEEE 802.11 lower-MAC port is built on the same core. | Keeping two identical owners duplicates fixes. Moving `B` into its owner loses the core's asynchronous abandonment authority. A receiver-based `drive` needs an empty driver slot. Flag-only retirement loses wakes. | Rework signal/future representation if the added code or installation RAM exceeds the target image budget; preserve proofs, wakes and asynchronous teardown. Re-measure this ELF and the affected firmware before accepting that rework. |

Most code is the concrete async state machines: the linked ELF attributes
2256 bytes to each backend's polling entry, 630 bytes to each body future,
440 bytes to shared installation drop glue and 516 bytes to the retirement
poll closure. These sizes include inlined wake handling; they are not an
ablation attributing a byte count to each guarantee. The extra future RAM
retains both sides of the body/retirement join, the driver loan and owners,
the deadline, and the three independently wakeable receivers. The call and
proof tokens themselves are zero-sized.

A candidate reduction is factoring backend-independent signal and join
code out of the two monomorphizations. It needs another ELF measurement:
LTO already shares some glue, and out-of-line calls can increase code or
execution cost. There is no established cheap saving to claim here. Removing
an abandon receiver would lose one of two independent wakes; discarding
join state would lose returned ownership or delay retirement behind the
body. Neither is an acceptable size reduction.

A backend's installation factory returns `Installed::new(backend, driver)`,
its driver lives outside the backend, and it implements static
`drive(DriverLoan, held, retire)` returning the two proved exits. Driver operations work through the closed shared view; the future
holds no hardware owners. Retire consumes the loan; a fault first publishes
poison and consumes both publication and loan in `DriverLoan::fail_stop`. Uninstall receives the retained driver with its
completion state in `Retired`. Hardware-reachable backing stays static,
outside the movable driver. Its branded queues
stay in `Held<'id>`; `resources` returns `InstallParts` instead of a tuple.
Backend calls take the core's affine `Call`; `enable` and `setup` also take
held/local installation loans. Uninstall consumes `Retired` and `Detached`. `retire_bound` returns a future without a backend borrow.
The disable sink uses the same `ControlFlow<Terminal>` rejection shape as
the Wi-Fi port: consumers that accept every terminal return `Continue(())`.
Only `next_event` has the outer error `EventEnd<Fault>`; other methods return
`PortResult`. The event consumer must remember its final variant and
cause across phase loans. StationService accounts every admitted terminal
before accepting `Drained`; a remaining identity signals a contract violation.
On `Quarantined` it leaves unfinished work with the core's quarantine and
fail-stops without further draining or disable. No event-end proof or owner
is transferred in the error itself.
`abandon` immediately quarantines and returns a proved `Stopped` or an
`Unconfirmed` outcome; final `detach` follows the body's return and borrows
that outcome through its own single-use call authority. Unconfirmed hardware requires chip reset and permanent backing
quarantine, not a runtime assertion that it stopped. `fail_stop`
returns `FailStopped`, which `Ended::fail_stopped(proof, output)` consumes.
`run_port` returns `PortEnd`; `StationService` supplies its result as
`PortBody::Output`. The generic phase, separate event/terminal types,
consuming owner transitions, `Failed::{Refused, Poisoned}` and `PoisonedOwner`
are part of the contract.
