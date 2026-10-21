# Radio port lifecycle and vocabulary

`oer-radio-port` supplies caller-owned lifecycle storage, slot identities,
capacity tokens, errors and clock relations shared by radio ports.

| Value | Meaning |
| --- | --- |
| `PortSlot`, `SlotClaim`, `VacantPortSlot`, `SlotOwner`, `SlotRegistration`, `SlotFrontend` | Permanent owner and queue backing, affine placement, and separate P2/P1 endpoints |
| `PortBackend`, `PortBody`, `Owner`, `Ended`, `PortEnd` | Public body views, consuming typestate transitions, a typed body result and the supervisor's actual lifecycle outcome |
| `Joined`, `SlotStopped`, `SlotQuarantined` | Ordinary reuse authority, a registration-specific P2 stop proof and a quarantine response without extraction or reuse |
| `SlotIdentity`, `LocalId`, `StaticId`, `Root<Id>` | Closed invariant identities independent of body loans. A type alone grants neither placement nor reuse authority |
| `Mint`, `PoolSet`, `PoolPath`, `Slots`, `Permits`, `Slot`, `Permit`, `PermitSupply`, `Seat`, `Line` | Complete affine capacity issuance and component paths. No capacity token is Clone, Copy or Default |
| `PortResult`, `Poisoned` | Commands and clock calls return `Result<Result<T, Refusal>, Poisoned<Fault>>`. The inner `Err` is a typed refusal (nothing changed); the outer one carries the backend's cause, which portable code passes on without interpreting it |
| `EventEnd<Fault>` | `next_event` alone returns `Result<Result<Event, EventsLost>, EventEnd<Fault>>`. `Drained { cause }` promises that every terminal of admitted work was delivered. A missing terminal violates the contract and can be detected by the body. `Quarantined { cause }` leaves undelivered owners in the slot's permanent quarantine. The end contains only its cause, no owners or affine proofs, and every subsequent call returns the same variant and cause |
| `EventsLost` | Events were dropped. Reported once, in place of the first dropped event; events before it precede the gap and events after it follow it. A backend reserves at admission the slot of every terminal event and of data its protocol promises to deliver (acknowledged Bluetooth LE connection data), so the gap holds only data the protocol does not promise (received frames, advertising reports) and the consumer continues |
| `ClockInfo<D>`, `ClockRelation<A, B>`, `RelationRate`, `RelationSample<A, B>`, `Generation<A, B>`, `GenerationSource<A, B>`, `Stamp<A, B>`, `Projected<T, S>`, `ProjectionError` | One relation between two clock domains, typed by its source `A` and target `B`: identical, or affine within a drift bound. A `RelationSample` pairs one reading of each clock with its uncertainty (a target span) and its generation; `project`, `project_back` and `project_span` are the one projection both ways, returning the value and an uncertainty that adds the drift over the distance from the sample, as a span of the target domain. A coordinate of another generation is `StaleSample`, a result outside the range `OutOfRange`. A generation has no public constructor: the relation owner's `GenerationSource` opens an epoch and jumps at each break. `ClockInfo<D>` states a port's resolution and its relation to monotonic time (`monotonic`, `None` when none exists); `RadioStamp<D>` and `ClockSample<D>` are the radio-to-monotonic `Stamp` and `RelationSample` |


## Conformance

The `conformance` module (the `test-support` feature) holds the rule every
permit pool meets, `permits_serve_takers_in_arrival_order`: its pool's
owner (a backend's held part, a runtime) calls it with the pool, three
seats of its line and at most eight permits it holds aside.

## Component authority and cost

`Root<Id>` is the path of a standalone layout belonging to slot identity
`Id`. A tuple adds one `Child` per
component at each nesting level; custom factories preserve the path they
receive, including its `PoolPath::Identity`. The default root is
`Root<LocalId<'id>>` for the token's local brand. It describes a standalone pool,
not a way to erase a tuple component's identity. A line is a copyable receipt
for its waiter domain. A FIFO's capacity comes from its consuming supply;
several supplies can deliberately use one line while their permit paths
remain distinct.

`Mint<'p, Layout, Id>` issues `Layout::Pools<'p, Root<Id>>`. The token loan
`'p` and the path's identity are separate invariant parameters: two tokens
with the same loan, kind and component index still differ when their slot
identities differ. A static identity does not make a token loan static,
including when a caller attempts to erase its type. Paths and identities
occupy no runtime storage and use no registry or claim latch.

| Consumer | Authority retained | Runtime cost of the path | Construction and cancellation |
| --- | --- | --- | --- |
| Wi-Fi | Gate and issuer, TX and Block Ack tickets, buffer/key pools, station connection source and its contexts | Zero additional bytes; indices remain `u8`, the epoch remains a `u64` | The phase accepts its associated gate path; terminals retain the submitted slots and connection context |
| Bluetooth LE | The backend's declared layout; the port test backend has a standalone slot component, production layouts without pools use `()` | Empty layout and Mint remain zero sized | Consuming issuance introduces no waits or cancellation state |
| IEEE 802.15.4 | Nested interface/configuration/pending-table paths and the separate radio-operation path | Zero additional bytes; each slot/permit/seat remains one byte | Configured and pending-entry owners retain their slot; operation guards release only to their source pool |
| FIFO runtime | Independent permit and waiter paths; opaque initial supply | No registry, allocation or new synchronization; line receipt remains zero sized | The whole supply constructs one pool; cancelled takes release their queue place and keep the borrowed seat |

Contract misuses and their positive twins live in `tests/ui/` and the
consumers' corresponding directories; each misuse must fail to compile on
host and RV32. Unit tests cover shared waiter lines,
cancellation, empty layouts and all 256 indices.


## Lifecycle and cancellation

A claim places one `PortSlot<Id, State>`. `with_local_slot` supplies a fresh
invariant `LocalId<'scope>` without a global declaration latch; repeated,
nested and parallel model scopes remain independent. The executor's
`declare_port_slot!` supplies one nominal `StaticId<Declaration>` and one
irreversible `take_static()`. Neither reset nor quarantine reissues that Id.
Unclaimed `VacantPortSlot` backing cannot register an owner.

`PortSlot::register` stores the P2 owner and arms its cleanup guard before
calling `prepare_queues`, splitting endpoints or constructing the body.
Queues have const vacant backing and are prepared through `&mut`; they do
not travel inside the body future. Registration returns a small pair of
`SlotFrontend` and `SlotRegistration`. The frontend contains only queue
endpoints and lifecycle signals. Its Drop atomically requests quarantine
and wakes P2; it does not call hardware or destroy the P2 owner.

`LocalExecution::scope(registered, body)` accepts that checked pair, constructs
the body once and polls P1 beside P2 without requiring Send. Its pinned fields
retain one body future; when the body returns, its storage holds the output
while registration ends. Dropping even a never-polled runner ends P1 before
the P2 cleanup guard. The current S31 conformance composition runs both sides
in its P2 task. Execution adapters establish the synchronization and transfer
bounds needed when the sides run in different contexts.

`SlotRegistration::run` prioritizes quarantine before each device poll.
Normal retirement consumes the registration's `SlotStopped` proof, finishes
and destroys the owner at its permanent address, and ends its invocation.
The completed P2 future is destroyed and unregistered before `Joined` is
published. Only that affine authority permits `PortSlot::reinstall`; stopped
hardware stays in the slot until P2 consumes it for the next installation.
No frontend result contains the backend or its hardware owners.

Quarantine is absorbing, including after a late P2 proof. P2 executes cleanup
exactly once and retains every reachable owner in durable backing.
`Abandon::Stopped` requires its actual hardware stopping proof;
`Abandon::Unconfirmed` requires chip reset. Neither authorizes reuse of the
quarantined allocation. `PortEnd` reports the timely result through
`SlotQuarantined`; dropping that receipt destroys no hardware owner.

`SlotBodyOwner::lend` introduces a separate invariant body loan `'p`, even
for StaticId. `BodyLoan` privately converts complete owned Id payloads to
and from public views, preserving component path, kind and index. Queues
hold owned Id payloads and no reference into a P1 future. Neither a body
resource nor its protected context can escape through `Box<dyn Any>`.
`PortBody::Output` is selected before this loan exists.

`PortContext` fixes its owned representation before introducing the loan.
Domain contexts expose only their declared `ContextProjection`; protected
proofs retain the loan in borrowed and consumed projections. Moving a whole
context through a queue does not expose its owned representation to a body.

Enable and disable consume `Owner`; setup and phase borrow it through an
invariant `Lent`. Refusal preserves disabled authority, while poison retains
its cause in `PoisonedOwner`. Disable delivers every admitted terminal to its
sink within the backend's transition bound. A sink accepts with
`ControlFlow::Continue(())` or returns the whole terminal with
`ControlFlow::Break(terminal)`; rejection retains that terminal and every
remaining owner in the backend's quarantine.

`Owner::fail_stop` synchronously publishes cleanup and returns
`FailStopRequested`. `Ended::fail_stop(request, output)` carries that request,
not a claimed hardware outcome. `Ended::uninstall(owner, control, output)`
consumes disabled or poisoned authority. Forgetting authority cannot create
Ended or a stop proof.

One lifecycle supervisor in `run_port(frontend, body)` ends the body loan and
waits for P2 under the named `shutdown_bound`. The bound does not borrow raw
frontend state. Deadline checks bracket the answer poll: a late ready answer
cannot produce Stopped or Joined. Expiry returns Unconfirmed and leaves
quarantine absorbing. The supervisor polls even when the body awaits an
unrelated future; cancelling it only requests P2 cleanup. S31 uses its
existing `SHUTDOWN_BOUND` and exact synchronous radio clock sampling.

`PortEnd::Uninstalled` carries Joined and the body output. `FailStopped`
carries the actual timely P2 outcome and output; `Abandoned` carries the
outcome when no Ended result survived. Body state is never an alternate P2
cleanup guard.

The final `EventEnd` remains stable across phase loans. An ordinary fault
can return Drained only after all admitted terminals were delivered.
Accounting quarantine preserves missing owners and ends delivery immediately;
it does not itself decide whether later P2 cleanup can prove Stopped.
StationService accounts every admitted terminal before accepting Drained and
fail-stops on Quarantined without another drain or ordinary disable.

Local models can end their entire hardware world through `ModelSlotOwner`.
`end_model_world` consumes the local slot after every endpoint and P2 loan
ended, removes modeled reachability and then drops retained owners and queues.
It returns neither an owner nor reuse authority. Static slots have no such
operation.

## Cost and checks

The core allocates no heap memory and contains no unsafe source code. Slot
identities and component paths occupy no storage. Initial capacity is issued
once per registration; const queue backing remains in its caller's slot.
P2 owns the cleanup guard. The body's fail-stop request is zero sized, and
only the supervisor stores the bounded response wait.

Pin projection keeps the body at one address and reuses its storage for the
returned output. Separate poll boundaries keep body and registration frames
independent. Linked image analysis measures their actual compiler frames,
future backing and all allocated sections, including one-time initialization.
The [cost probe](examples/lifecycle_cost.rs) exercises two concrete backends.

UI cases and their positive twins cover closed identities, affine placement,
foreign paths and resources, body-loan escape, quarantine and Joined. They
compile on host and RV32. Unit tests cover cancellation before and after
first poll, end-of-loan ordering, P2 destruction/unregistration, deadlines,
late proofs and independent repeated/parallel local scopes.
