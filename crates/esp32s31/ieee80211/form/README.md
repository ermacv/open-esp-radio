# ESP32-S31 Wi-Fi execution form

`oer-esp32s31-ieee80211-form` is the runtime-layer ownership prototype for a
separate S31 driver. It depends on hardware owners and portable contracts;
it has no dependency on esp-hal or a composition. The adapter supplies the
routing authority and context-free ISR entry. The adapter-to-form dependency
is provisional until review of the form.

The portable entry consumes `Installed<B>`, with S31 Driver as `B::Driver`,
and branded state belongs to `Held<'id>`. The lifecycle core owns Driver and
lends it to the driving future. The linked fixture retains `RadioRuntimeOwner`
by value in permanent backing.
The portable entry is an integration boundary; it cannot add a second IRQ
registry or change the backing's reclamation proof.

`scope::driver::Driver` is the unbranded installation owner. Its `drive`
method borrows it through the core's single affine DriverLoan until retirement;
that future owns neither
Driver nor the backing lease. The executable binding in
[`scope/tests/lifecycle.rs`](src/scope/tests/lifecycle.rs) uses the real
shared `Installed`, `run_port`, `PortEnd`, `FailStopped` and `Abandon` types.
Its frontend models lifecycle only; the S31 backend (S31-1 of ) binds it
to the port.
The fixture accepts the core's `ControlFlow<Terminal>` disable sink and
`EventEnd` event-stream result. It admits no radio work, so terminal retention
and event-stream draining belong to the S31 backend's conformance tests.
The independent linked hardware fixture retains `RadioRuntimeOwner` by value.
The tests retain a non-static context and an affine installation slot in
`Held<'id>`, and never register an address into Held with either executor.

After the driving loan ends, consuming `Driver::finish` returns
`State::Released` or `State::Quarantined` explicitly. The latter retains
`ScopeReturn` in `Quarantined<S>` and has no owner or lease extraction. On
`Abandon::Unconfirmed`, the core must instead retain Driver itself without
calling `finish` or its destructor. The synchronous backend abandon path has
already masked the inputs and closed admission. The entered ISR or step can
finish against permanent backing even if the driving future and the outer
container holding the quarantined Driver are destroyed. There is no registered
address into Driver, its future, Held or the result latch.

The shared binding consumes `DriverLoan<'p, Driver>`, whose dereference gives
only `&Driver`. It awaits `retire.wait()`, then executor retirement, ends that
borrow and calls `retire.requested(loan)`. Ordinary uninstall receives the
retained Driver alongside its completion state and consumes `finish` there.
`PortEnd::FailStopped` and `Abandoned` carry the physical abandon outcome and
the core's quarantine receipt. Cancellation closes the same scope admission
domain before the core quarantines Driver without Drop.

Driver and returned state use `oer_radio_port::Quarantined<S>` directly.
There is no local type, alias or extraction method. The standalone scope's
`ScopeReturnLatch` only delivers its synchronous outcome; it is distinct from
the common core's abandonment signal. An owned Driver's destructor remains a
safety net outside core quarantine. The core calls backend abandon at most
once per installation; there is no shutdown memo in backing. Normal detach
masks and closes the entry without extracting memory; consuming finish then
joins actors and stops once. A quarantined detach only repeats mask/close,
never stop or owner recovery.

Every await in `Driver::drive` preserves a state accepted by retirement,
abandon and consuming finish. Cancellation only ends the loan: it cannot
take a descriptor, queue or hardware owner out of backing. The two suspension
points wait for the core's retirement request and the executor's retirement
completion. A parameterized host test uses the real Retire from run_port,
cancels before polling and after 1–3 polls at each await, then exercises finish
with ordinary, retirement and abandoned-state handling. Eight further cases
cancel the whole run_port at those same cuts: physical abandon stops exactly
once, and core quarantine returns no lease or driver owner. The bounded DMA
ownership model verifies
that each published address retains its exact queued buffer, successful
teardown clears every descriptor and queue, and each buffer returns once.

## Ownership and return

```text
composition: one permanent SRAM backing and its ISR address slot
  ScopeStorage
    ├─ Header: exact callbacks, separate driver and IRQ addresses
    ├─ DriverRegion: by-value hardware and exclusive IRQ consumers
    ├─ IrqRegion: owned IRQ banks, route, scheduling and atomic signals
    └─ IrqStorage / PowerIrqStorage: facts, wakers, optional diagnostics

caller: DriverScope::run future
    ├─ affine pinned lease of that backing
    ├─ borrowed ScopeReturnLatch, never reachable from registered callbacks
    └─ arbitrary Context, body future and its retained result

common-core binding: Driver owns the same scope lease and latch loan
    driving future ── exclusive loan ──▶ Driver
    Unconfirmed ──▶ quarantined Driver, no Drop and no backing recovery

ScopeSlot ── admission counter + atomic address ──▶ Header
    driver executor ──▶ DriverRegion
    MAC / POWER ISR ──▶ IrqRegion (contains no driver address)
```

The composition declares the backing in SRAM. At boot it transfers its unique
`&'static mut ScopeStorage` through `pin_static`, following the permanent-pin
pattern used for BLE. There is no allocator, shared `take` method or runtime
"already taken" flag. The lease is neither `Clone` nor `Copy` and never exposes
a mutable reference to its backing. `Released` returns that same lease for a
subsequent installation. `Poisoned` never returns it; only chip reset permits
another installation. The scope owns the lease, not the allocation. In the
common-core binding that lease remains in Driver while its driving future
only borrows Driver.

This changes the original §3.1 inline-storage proposal. A bounded timeout in
`Drop` cannot make inline storage safe: dropping `Pin<Box<Future>>` would free
the registration while an entered actor still uses it. Permanent backing
survives owning-future drop, `forget`, and timeout. The reclamation regression
holds an actor on a second core, drops the owning pinned future after timeout,
then lets the actor finish against its still-live backing.
The separate forget regression pauses MAC ISR, POWER ISR or a driver step,
forgets the owning future, then resumes that actor. Registration identity and
queued buffer owners remain unchanged, no lease returns, and body, hardware,
IRQ and buffer drop counters all stay zero.

The body runs on the caller's executor. Scheduling only pends a separate
driver step. The body moves its result into consuming `ScopeAccess::retire`, which requests
retirement and retains that result until the driver accounts for its work.
Detachment masks MAC, POWER
and driver-SWI, closes admission, waits for entered actors, and calls the
synchronous `stop(&mut Driver)` exactly once **only after successful drain**.
The successful output contains a returned lease and owners chosen by the
fact `stop` returns, not by the path: `Stopped` owners after a quiescent
stop (retire, `ScopeAccess::abandon` or cancellation alike; the core's
fail-stop still quarantines, ), opaque quarantined owners
after an unconfirmed one. Quarantined owners expose no
restart, clock-release or extraction operation. IRQ `quiesce` consumes its
epoch once; there is no `AlreadyQuiesced` state or refusal.

Explicit fail-stop consumes `ScopeAccess` and completes that synchronous
mask/close/wait path before returning to a possibly still-pending body. Its
active scope owner is consumed into `ScopeAbandoned`; the outcome has already
moved into the result latch. A body pending after that transition owns no
lease or shutdown guard, so cancellation cannot stop again. Cancellation
while access is active uses the same path from `DriverScope`. There is no
`Cell<Option<Quiescence>>`, replacement flag or cached shutdown outcome.
An empty latch means no output was delivered, for example because an owning
future was forgotten; it does not claim successful release or poisoning.
Context needs no `'static` bound and is unreachable by the registration.
Forgetting a borrowed `Pin<&mut Future>` leaves the future's own destructor
responsible for cancellation. No lifetime is extended or transmuted.

## Production S31 integration requirements

No production composition opens the form's registration yet: the old S31
runtime, composition and HIL consumers were deleted (), and the S31
backend that replaces them is S31-1/S31-2 of . The esp-hal adapter's
`service_mac_interrupt`/`service_power_interrupt` enter the `INSTALLATION`
slot; while it is closed, an entry returns
an empty snapshot without servicing hardware. The backend constructs and runs
the owned Driver with that exact slot and permanent backing and borrows its
installation's diagnostics. No compatibility storage may reopen another path.

The pinned TX pool issues claims only through `PinnedDmaTxPoolHandle`, the
affine result of `pin_static`. A raw pool or an independently pinned borrow
has no claim method; host/RV32 UI twins check both boundaries and handle
reuse. Permanent allocation is now a type proof, independent of the existing
DMA guard assertions. Guard/state/length assertions detect DMA
damage or violations of the existing slot protocol, not missing pinning.

## DMA custody and clock release

`StableDmaBacking` requires both the allocation and its exclusive reservation
to survive a leaked lease. A borrowed stack buffer cannot satisfy that unsafe
contract. The pinned TX pool consumes permanent storage; its leaked radio
lease leaves the slot reserved. The tagged and returning wrappers preserve
this guarantee while allowing shorter-lived CPU-only metadata. Retained
A-MPDU storage keeps its backing owners in `ManuallyDrop`, so forgetting the
DMA owner and then destroying its arena cannot free those allocations. Host
regressions read the retained bytes after the metadata and arena end.

A stop that read DMA quiescent (`StopOutcome::Quiescent`) on any of the
form's own paths (`ScopeAccess::retire`/`abandon`, a cancelled
`DriverScope`, a lost `Driver`) recovers the by-value radio owner and
inactive IRQ setup; an unconfirmed one quarantines them. The core's
fail-stop and whole-run cancel (`oer-radio-port`) still keep the Driver
quarantined even after a quiescent stop; that is 's work. Until the HAL
reading lands every production stop is unconfirmed.
The resulting stopped installation may consume its Wi-Fi clock obligation
through `WifiClocked::release_clocks`, whose exact result is
`Result<ClockReleased, (StoppedInstallation, ClockPoisoned)>`. Success moves
on to common-power release once. Any failed release, physical or the shared
modem planner's internal refusal (`Rejected`, `NotEnabled`), retains both
stopped register custody and the unresolved obligation; there is no retry
method or replacement `WifiClocksOn`. Host/RV32 UI pairs reject repeated
release and release through poison. The shared modem planner's recovery remains .

This clock failure follows an already stopped installation. Scope timeout
instead returns `ScopePoisoned`, without a stopped owner or access to any
clock-release operation: entered actors or hardware may still be running.

## Command ownership, stages and ordering

`scope::commands::CommandStorage` owns two SPSC stores in Held. Its unique
construction authority retains the same `T` and `K` as `CommandPools<T, K>`;
an authority for larger pools cannot initialize a smaller store. The only
producer methods consume their actual affine command admissions. There is
no `Full`, `try_send`, second producer, or generic public queue insertion.

| Store | Capacity | Admissions retained by commands |
| --- | --- | --- |
| Setup | `2 + 1 + 1` | STA/AP configure, retune, Enable |
| Phase | `T + 2*K + 2 + 2*2 + 2 + 1` | TX tickets, pairwise install/remove, GTK mutation, STA/AP configure and TSF, gate, Disable |

The phase formula is the lower-MAC command bound plus the distinct lifecycle
Disable place. Each interface contributes **both** its `ConfigurePermission`
and `TsfPermission`; configure is a FIFO command carrying its exact receive
value and opaque context. `Configuration::configured` and `Sampling::sampled`
construct the actual `Configured` and `TsfSampled` terminals, returning those
same permissions. Configure is never placed in a coalescing settings cell.

Enable is the setup sender's final publication and consumes that sender into
`PhaseSender`; the receiver changes only when it consumes Enable. Disable
does the reverse and leaves a `DisablingSender` with no enqueue methods.
The Disabled reply retains both lifecycle places until it is consumed to
recover SetupSender. S31's enable operation is synchronous within a bounded
hardware step, so there is no hardware `Enabling` owner containing invented
pending data. Waiting for that step's reply still belongs to the common core's
consuming enable transition.

Closing the gate transfers its owner into the FIFO. Only a dequeued
`ClosingGate` can make the barrier reply. `open_gate` first publishes its
internal OpenGate command and only then returns GateOpen. At most that Open
and the next Close can coexist; no external caller can forge a barrier reply
from an Open owner and exceed those two places. FIFO order puts the Open
before every subsequent Submit, and Close after every earlier Submit.

Submission contains the complete lower-MAC `InFlight<C, Ticket, M>` and the
actual S31 `TxAdmission` (`KeyUse` in its protected variant). Queue movement,
execution projection and terminal preserve context, ticket and whole MPDU;
there is no separately indexed body collection. The full-capacity regression
fills all 15 places for `T=2, K=2` before receiving, including two accepted
MPDUs with two independent bodies each. It verifies original context and
body addresses, SN and PN through the public Attempt/InFlight/Completed API.

Coalescing settings and BA state require one SPSC triple buffer per setting
kind: producer-private back, consumer-private front, and an atomically
exchanged middle. Publishing the latest complete value uses Release before
publishing the next FIFO command, then wakes the driver. The receiver must
**Acquire the command first**, acquire each changed settings cell, and apply
the resulting values before executing that command. Reading cells before
the FIFO Acquire would leave a race even if each cell were individually
atomic. The current `InterfacePhase::{add,replace,remove}_block_ack` contract
also specifies desired state, not commands or asynchronous replies. Each BA
slot therefore has its own triple buffer; its affine agreement retains the
slot while installed in the caller's desired state. Removal returns the slot
immediately, and reuse before the driver runs replaces that pending removal
with the new desired agreement. No intermediate hardware installation is
promised. BA adds no FIFO capacity term. An operation with an individual
reply would instead need its own affine request credit and a FIFO command;
the current BA API has no such operation. This ordering is the required S31
binding.

Each permission's place is its memory credit. `AffineIndexedSpscQueue` in
`oer-memory` keeps one value cell per place and the existing audited
`AffineSpscQueue` as a ring of indices. The form implements the unsafe
`UniqueQueueIndex` contract on a private command envelope: the sole Mint and
disjoint ranges establish index bounds and uniqueness. Configure/TSF places
are fixed by their concrete interface types; TX reads `InFlight::ticket()`,
and key commands use their `Slot::index()`. There is no contract change,
separate credit object, runtime lookup/pop, capacity refusal or extra await.
The senders retain T/K as const parameters, with no runtime geometry fields.
Every cell range starts after the preceding minted range. Compile-time checks
bind each interface's exported COMMAND_CAPACITY to its mapped configure/TSF
permissions, check singleton widths, and equate the final range ends with the
two allocation capacities. A changed lower-MAC total therefore fails compilation
until the mapping is updated. A bijection regression covers every command and
all issued indices, including empty pools and the full 0..256 mint domain.

Memory proves that a producer holding one of N unique places leaves at most
N-1 indices in the ring. Release publishes the complete value and its index;
Acquire receive finishes the move before its permission can return through
the response. That transfer orders reuse across cores. OpenGate's two-place
proof also uses FIFO order: the next Open needs a Close reply, whose receipt
follows receipt of the previous Open. Only typed sends construct envelopes;
the public unit OpenGate variant alone cannot authorize a memory send.
Dropping a store destroys at most its fixed capacity. Registered callbacks
never contain its address: a timed-out external step cannot keep a reference
into Held after the common core frees its own future. Tests exercise full
occupancy, reuse and response interleavings on another core; UI twins reject
send without the audited permission bound.

### Direct publication to a separate executor

`scope::commands::bridge` is the command-boundary prototype. It does not
implement `PortBackend`, endpoints or retune hardware. The only public
binding is `Bridged::bind(hardware, authority, storage)`: it consumes one
command authority and one `&'static mut ExecutionStorage<S, P>`, returning
caller-side Held and the `Bridged` composite, which owns the driver's
hardware and the executor. Only permanent records, private place names and
their SPSC endpoints cross to that executor;
Context, complete InFlight, MPDU leases and KeyUse remain in Held. Context may
borrow a non-Send value. A payload may likewise retain a short CPU-only tag:
`ProjectExecution::project` takes its narrow GAT view as an associated
function, avoiding a higher-ranked GAT bound that would imply `M: 'static`.
The UI twin instantiates the sender with that borrowed payload and rejects
passing its mutable owner to projection. No future or Held address is published.

The phase method publishes synchronously: retain the whole command by its
place name, project execution data through its narrow view, initialize the
permanent record, publish its name Release, then wake. There is no first
command queue or forwarding pump in drive. `CommandStorage` above remains
the local queue harness for the admission/index proof; a direct binding does
not allocate it. The caller's reply wait acquires a completed name, removes
the record, and takes exactly the original owner. No step reconstructs an
InFlight from Context, ticket or separate body slots.

`IndexedCustody` in oer-memory provides the waiting places. Insert consumes
the unique private name and returns its only removal name. The record store
retains that name and yields a second affine name; the same second name
travels through request, finite step and reply, and is consumed to recover
the first. Names contain only their index. The private `CustodyDomain`
implementations and paired endpoints prevent mixing identical layouts from
two installations; the index alone is not this association proof. There is
no RefCell, Cell<Option>, free-credit pool, occupancy admission check or
async send-permit. The ordinary end is
`Held::finish(&mut executor, &DriverStopped)` after proven DMA quiescence
on any of the form's own stop paths: consuming Held ends every producer, then the executor
withdraws every pending record. It returns every retained owner; destroying
a non-empty store only quarantines its values (erasing keys) and leaks them,
never dropping an owner.

Both setup and phase keep their original S/P capacities and FIFO order.
Enable is the last setup record; the executor then services phase. Disable
is the last phase record; its reply follows every accepted record. A phase
method publishes settings/BA before its command; the executor must Acquire
the command before reading desired state. The model tests that ordering on
another thread. The actual triple-buffer settings/BA binding still belongs
to the shared-loop integration.

Key commands and permanent records both own the common
`oer_ieee80211_mac::ccmp::Ccmp128Key`. Admission moves the key into the command
in Held. Projection initializes the record's key through
`Ccmp128Key::new(key.as_bytes())`, borrowing the Held owner, then zeroizes that
owner before publishing the request. No owned byte array crosses this
boundary. The executor borrows the record's `as_bytes()` only for programming.
Immediately afterwards it calls `Zeroize` **on that owner in the permanent
record, before publishing its reply**. Withdrawal erases the same cell
without programming hardware and returns `Withdrawn`, never `Applied`.
The original permission returns from Held with its key already erased; a
retry must supply new material. Normal key destruction also zeroizes storage.
`Held::finish` and the executor's drop withdraw pending records and erase
their keys. After every stop whose drain completed, whatever DMA fact it
returned, the form itself withdraws every pending record of the executor
inside the driver's `Bridged` hardware and zeroes its key, on every stop
path, before any owner can be quarantined ( ; erasing after
both DMA facts at the stop call is the owner decision ). No receipt is
involved: the sealed `OwnsDma` tells the form where the executor is (UI
pair `owns_dma_sealed`). Two cases leave record copies until reset: a drain
timeout (`ScopePoisoned`), where no stop runs, and a key command the caller
still publishes through Held's endpoints after the stop, because the
quarantined executor never withdraws it: the senders are not closed at the
erase, so a composition must not publish after a stop. This does
not promise erasure of compiler temporaries created by moves.

The key regressions pause before reply publication and inspect the actual
record for both PTK and GTK, on both apply and withdrawal paths. They also
cancel a reply wait at every applied/pending key frontier and check that
withdrawal preserves the applied prefix and returns every original permission.

Every await in the bridge is a reply wait holding only a mutable endpoint
loan. Cancellation leaves the operation in Held and the execution record in
permanent storage, in a state which finish/retire/abandon can account for.
After a stop that proved DMA quiescent (`DriverStopped`, on any of the
form's own stop paths; not yet through the core's fail-stop or whole-run
cancel, ),
`Held::finish` returns every remaining owner.
The executor's drop and the form's erase after a stop return queued records
as `Completion::Withdrawn` without applying them; already completed replies
retain `Completion::Applied`. Withdrawal cannot fabricate a TSF sample. A reply
distinguishes `Applied<T>`
from `Withdrawn(T)`. Applied's constructor and fields are private; only an
applied CloseGate/Disable reply can issue GateBarrier/Disabled. Taking the
original owner consumes its Applied receipt. A withdrawn command cannot be
substituted for this proof. UI twins reject both substitution and forged
Applied construction; a regression withdraws CloseGate and Disable without
executing either.
`Held::finish` consumes Held, so producers have ended, and borrows the
executor, so no worker step runs. The form's erase after a stop runs with
exclusive access to the executor (every actor drained), not after producers
ended; the executor's drop likewise. After
an unconfirmed stop Held is quarantined: dropping it, wrapping it in the
core's `Quarantined<Held>` or forgetting it leaks every owner and never runs
a Context/MPDU destructor; a reply future cannot free the permanent records.
`Held::finish(&mut executor, &DriverStopped)` is the only release: it refuses
another binding's executor with `ForeignExecutor` (pointer identity of the
record stores), takes every replied record back out of the static storage
(`HeldOwners::records_taken_back`) and yields the owners in `HeldOwners`.

The DMA brand ( , , ) is the DMA owner's
type `D` (production `RadioRuntimeOwner`, the S31's one Wi-Fi DMA). Outside
the form, `Bridged<HW, S, P>::bind` is the only way to bind a Held: it
returns `Held<…, HW::Dma>` and the composite that owns `HW` and its
`Executor<HW::Dma, S, P>`. What the types enforce:
- the executor steps only through `Bridged::step`, on the DMA owner of the
  `HW` it is installed with (UI pair `bridge_step_through_composite`);
- `Held::finish` takes only a `DriverStopped<'_, D>` for its own `D` (UI
  pair `bridge_finish_foreign_dma`);
- the proof borrows the stopped owners, and `StoppedOwners::parts` lends
  only the executor beside it, never the hardware, so no step can restart
  DMA before finish releases the frames (UI pair
  `stopped_proof_excludes_dma`);
- only the form implements `OwnsDma` (UI pair `owns_dma_sealed`).
They do not make `D` a singleton: that `RadioRuntimeOwner` has one value
per image is the HAL's ownership, and two bindings for one `D` are told
apart at run time by `ForeignExecutor`.
The test pauses a step on the second thread across those transitions and
checks zero drops and three still-reserved pinned TX allocations. Ordinary
Held destruction alone is **not** a hardware shutdown operation.

The model has no MMIO or DMA dereference. Its TxImage describes three
fragments (header and two bodies), projected from actual pinned TX leases
through ExecutionPayload; this is a fixture descriptor budget, not a
production chain limit. Address metadata is not an independently usable
DMA capability. Starting real DMA and proving its terminal remain S31 work;
model completion is not a substitute for that proof. Multi-descriptor MPDU
and CCMP-on-chain behavior still require HIL. The model's static execution
store is consumed once and has no recovery/reinstall method; integrating
reusable Released backing remains part of the production installation.

The RV32 link fixture places the entire execution store in
`.critical.data.open_radio_commands`; the ELF audit checks its full range in
SRAM and inside the `__runtime_critical_data_*`/`__runtime_critical_bss_*`
ranges the staged runtime entry re-copies or zeroes at every reset. Any
production `ExecutionStorage` placement must keep that property: pending record key copies
left by a drain timeout must not survive a reset ( ). It is
separate from the exclusively borrowed driver region. For
`T=2, K=2, S=4, P=15`, the record/queue backing is **2688 B**, independent of
future or Context size. The caller-side representative Held is **548 B**
(context word and three-word MPDU owner), and the executor is **44 B**.
These are ABI measurements, not production memory budgets. Host tests use
real three-lease MPDUs and report their separate host sizes. Compared with a
forwarding pump, direct publication removes S+P initial FIFO cells, two
order rings and one scheduled handoff; it keeps S+P retained-owner cells,
S+P record cells, four order rings and two projection function pointers.

Host `command_to_step_host_latency_fixture` reports 256 command-to-step
samples using the actual request/reply wakers, including OS scheduling. These
values neither set DrainBound nor
bound RF retune. Production maxima, SRAM budgeting and the whole executable
driver graph require the S31-2 measurement profile and HIL evidence.


## Shared lifecycle boundary

The shared contract lends both `&Held<'id>` and `&mut Local<'id>` to setup,
enable, phase and disable. Branded queues remain in Held and sender accounting
in Local; neither is stored in the backend before the installation brand
exists. `tests/contract_boundary.rs` compiles the previously failing enable
and setup calls against these real signatures. The lifecycle witness uses the
actual core entry and loan; its frontend models lifecycle only. The hardware
binding of these typed queues remains the S31 migration step.

## Quiescence, poison and priorities

Each ISR or driver entrant increments the shared counter **before** checking
admission. Closing the gate and observing the counter use sequentially
consistent atomics: an entrant not seen by the zero observation must see the
closed gate and cannot project the registration. Detach masks all hardware
routes synchronously, closes admission and observes elapsed monotonic time.
`DrainBound::from_maxima<STEP_US, IRQ_US, MARGIN_US>()` sums the maximum finite
step, complete IRQ path and positive scheduling/clock-resolution margin. The
route implements `Clock` through the composition's callback; that source must
continue advancing with these IRQ inputs masked, on every participating core,
and its read cannot wait for them. Spin count and CPU frequency do not define
the deadline. It acknowledges driver-SWI without taking the IRQ-bank lock,
so a stuck actor holding that lock cannot block the timeout path itself.

Successful drain excludes every prior actor on every bound core. Only then
may the scope borrow the driver for `stop`, detach register owners, destroy
its private handoff borrows and return the lease. A late rejected entrant
accesses only the permanent address slot. Lease reuse initializes fresh facts,
wakers and diagnostics before opening the next installation.

`Poisoned` means **masked inputs, closed admission, permanent DMA and owner
quarantine**. Hardware may still be running. It is not `HardwareStopped`,
and its type exposes no stopped owner, lease or clock-release operation.
Timeout performs no driver borrow and calls no `stop`. Later actor completion
cannot turn poison into release. The composition must report the fault and
arrange a chip reset; it must not attempt reinstallation or clock release.

Driver steps process a finite IRQ batch and bounded command work, returning
`WakePlan::Ready` or specific sources and a future deadline. A step timeout
therefore indicates a hang or defect, not ordinary backpressure. Known causes
to investigate are a non-returning MMIO/device operation, a step or scheduler
violating its finite-work contract, a stopped core, and an IRQ-bank critical
section whose owner never exits. A caller preempting an actor can also exhaust
the time budget if it prevents that actor from ever resuming; quarantine
preserves safety for that order too. The production S31 step must measure and
bound step/IRQ duration and scheduling latency, including cross-core locks,
and choose a margin before enabling an image. No production values are
supplied by this prototype. Virtual-clock tests cancel during an entered step
on another core and get Drained at three observation rates; every successful
path stops exactly once. Deadline expiry calls no stop and retains backing.

Recommended priorities for latency are hardware ISR above driver-SWI above
caller. This is a performance recommendation, **not a safety condition**.
The same admission/quarantine proof covers the opposite order and execution
on another core. Unit tests control both same-core preemption orders and
cross-core overlap, detachment during either actor, successful drain and
cancellation during an ISR. Composition still serializes driver steps across
cores, and its cross-core critical section serializes only the disjoint IRQ
register bank, never a borrow of Driver.

An independent emergency MAC stop or reset remains an open HIL question.
If a hung step can leave the transmitter active, composition will need the
BLE `EmergencyStop` pattern: a separate owner of only its own registers,
without `SharedRadioLease`. This prototype does not claim that masking stops
RF transmission and does not invent a reset sequence without stand evidence.

## IRQ handoff and wake boundary

`capture_mac` and `capture_power` finish all register, fact and diagnostic work
before returning private, lifetime-free publication values. Those values own
only snapshots and `IrqPublication` with its `Waker`. They contain no driver,
slot, counter, backing or registration reference. Admission guards and IRQ
borrows end before `wake(self)`. The ISR cannot access installation memory
after wake by the structure of this tail, not by convention about line order.
Driver completion follows the same rule. Composition wakers, including their
clone and drop callbacks, may only schedule: they must not synchronously poll
or detach. Tests assert that both wake tails run after the admission count
returns to zero.

`IrqStorage::split(&mut self)` lends one affine consumer. Every wait holds its
exclusive borrow until completion or cancellation. MAC has one finite batch
path: RX success precedes TX complete, timeout and collision. New publications
remain for another step, so continuous RX cannot enlarge the captured batch.
Power has its own exclusive consumer. Register-before-check polling closes
publication races; cancelling one wait cannot steal the next wait's wake.
Moderation is `ActiveEpoch<Moderated>`'s type, and consuming `quiesce` returns
`InactiveEpoch`. No global register bank or fallback moderation callback exists.

`IrqDiagnostics<'_>` borrows its installation's counters. Without
`irq-counters` neither that diagnostic type nor its counter accessors exists.
`mac_irq_cycles` additionally requires RV32 cycle measurement and
`task-poll-telemetry`. It includes status, facts and counters,
**excluding external waker invocation**. No global performance snapshot
carries hard-IRQ fields: every IRQ counter belongs to the installation's
`IrqDiagnostics`.

The driver's AtomicWaker points only to the permanent ScopeSlot, never to
Driver, Held or a future. A wake enters the slot's admission domain and pends
the current scheduler; after close it cannot inspect an old registration.
Both MAC/power IRQs and software capacity publications use this same path.
The lifecycle is one monotonic atomic enum: Running → RetireRequested →
Retired, with an absorbing Abandoned transition from any state. A step gets
one enum snapshot; retirement completion cannot overwrite abandonment.
Each dispatch registers before its Acquire snapshots. After arming a Wait
plan it checks remaining facts and the lifecycle snapshot again, making new work Ready
for the next finite step. A scheduler must not clear an already pending SWI
when arming Wait; the executor acknowledges the SWI before dispatch instead.
Tests inject publication inside the wait-arm boundary and retain a cloned
waker across detachment to exercise its wake-by-ref, wake and drop paths.

## The single unsafe primitive

`scope.rs` owns the permanent backing lease and all registration projections.
Command-cell and ring access belongs to audited `oer-memory`; the form proves
only the permission-to-index mapping through its private unsafe trait impl.
Its unsafe binding constructor requires unique routing authority, synchronous
masking on every bound core, an exclusive driver executor, cross-core IRQ-bank
serialization, and bounded non-reentrant callbacks. Retained hardware is
`Send`; shared scheduler/platform data is `Sync`. Their reachable memory must
survive forget and pass the SRAM closure audit. Context and the result latch
are excluded from this graph. The exact callback and address pair is installed
together, so private casts recover only the correct region type. IRQ region
types contain no `DriverRegion` or hardware pointer. The admission proof covers
all such projections; there is no second unsafe global-storage mechanism.
See [UNSAFE.md](../../../UNSAFE.md).

Host and RV32 UI twins reject duplicate waits, setup activation or quiesce,
publication after quiesce, partial register ownership, route reuse, stack
backing, repeated lease transfer, and recovery of backing or stopped hardware
from `Poisoned`. Cross-core execution and reversed priorities are permitted
and tested behaviorally.
Additional twins reject setup Submit, phase Retune, any enqueue after Disable,
duplicate configure/TSF permissions, forged gate replies, mismatched mint
sizes, undersized storage, reused Driver, consuming recovery during the driving
loan, and extraction from quarantine. The no-Drop quarantine regression frees
the container holding Driver while an entered MAC ISR, POWER ISR or driver
step is paused on another core, then verifies that each actor resumes against
the permanent backing without owner destruction or a lease return.

## Observation: one MPDU across several TX-DMA descriptors

The inspected code establishes descriptor chaining, but does **not yet prove**
that one S31 MPDU may span a header buffer and several separate SRAM body
buffers, or that hardware CCMP continues across those descriptor boundaries.
These are open HIL questions; this form adds no such hardware implementation.

The [DMA descriptor](../../../hardware/esp32s31/driver/ieee80211/dma/src/descriptor.rs#L20)
has a next-node address, and
[external A-MPDU publication](../../../hardware/esp32s31/driver/ieee80211/dma/src/tx_ampdu_storage.rs#L1088)
links retained allocations. However, the existing
[A-MPDU commit path](../../../hardware/esp32s31/driver/ieee80211/mac/src/tx/ampdu/commit.rs#L37)
places an eight-byte metadata prefix, an entire encoded MPDU and its MIC/FCS
space in each contiguous allocation. Its existing `SOURCE` cites complete
`libnet80211[ieee80211_output.o]::ieee80211_alloc_tx_buf` (cache-TX/type-nine)
and `libpp[pp.o]::ppAssembleAMPDU`: retaining and linking those ESF buffers
proves the aggregate's no-copy ownership path, not a descriptor continuation
inside one MPDU. The
[length calculation's sources](../../../hardware/esp32s31/driver/ieee80211/mac/src/tx/ampdu/length.rs#L203)
likewise account for complete PSDUs. Ordinary TX currently
[publishes one descriptor with a null next pointer](../../../hardware/esp32s31/driver/ieee80211/dma/src/tx_storage.rs#L232).

[Ordinary TX](../../../hardware/esp32s31/driver/ieee80211/src/ordinary_tx.rs#L1513)
reserves hardware MIC space and programs the hardware key selector; A-MPDU
metadata also includes per-MPDU hardware MIC length. These observations do
not establish the CCMP engine's handling of a split header/body stream.
HIL must distinguish a continuation of one MPDU from the next A-MPDU subframe,
verify the plaintext case, then CCMP decryption/MIC, one PN and one FCS across
multiple body buffers. Descriptor flags, alignment and supported split points
also remain to be established from complete vendor code or stand evidence.

The ownership form therefore keeps an opaque whole MPDU `M` inside `InFlight`.
`M` may own a header and several bodies with separate retained SRAM leases.
Admission, retry, terminal and quarantine move or retain that whole owner;
there is no single-slice or one-descriptor restriction and no reconstruction
from an independent body table. This leaves the hardware question open without
constraining the eventual memory shape.

## Linked placement and cost

`tests/placement.rs` links the actual HAL, MAC and scope code in
[scope_placement.rs](examples/scope_placement.rs). This fixture is never flashed or used as HIL
evidence. SRAM and cached ranges come from `platform/esp32s31/chip.toml`;
`tests/placement.x` states the production linker section provenance. Header,
driver and shared IRQ memory live in the permanent backing section. The entry
stub separately establishes an SRAM stack for the executor and ISR.

The ELF audit follows the transitive call graph, every retained waker vtable
entry and the private header callback families resolved through DWARF. It
rejects unresolved calls and walks relocations through callback data and
vtables, including the full containing object when LLVM merges globals.
It checks the whole backing symbol as well as stack addresses. Negative linked
twins move code, read-only data, mutable data, backing or stack to cached
memory; each must fail. Ordinary and diagnostic feature variants pass the same
audit. Composition must apply this audit to its actual executor binding.

Measured RV32 release fixture costs: the body keeps both command stores live,
configures an interface and performs Enable/Disable. The command layout uses
two TX tickets and two pairwise slots, a one-word context, and three words for
an MPDU's header and two body owners. This measures the form, not a hardware
scatter/gather implementation or the complete lower-MAC backend.

| Checks | Code size | RAM and CPU | Signatures | Breakage | Alternatives | Reversal |
| --- | --- | --- | --- | --- | --- | --- |
| Affine lease, IRQ consumers, epoch and FIFO credits at compile time; finite atomic admission drain and IRQ-bank transactions at run time | 5498 B executable sections; 5768 B with diagnostics; one monomorphization per scheduler binding | Fixed permanent backing 124 B per installation, 156 B with diagnostics; 12 B ISR slot; total writable statics 2833 / 2865 B, including the separate 2688 B bridge backing; local command stores 1088 B; fixture caller frame 3328 B, separate 4096 B stack; backing size is independent of the body's future size | `ScopeStorage`, `ScopeLease`, `DriverScope`, `Released` / `Poisoned`, borrowed `IrqDiagnostics`; no `Context: 'static` | Consumers must own their IRQ handle; global snapshots lose IRQ fields; poison cannot yield hardware or a lease | Inline future storage cannot survive a timeout followed by free; priority ordering cannot prove cross-core quiescence | Reject bindings whose linked graph leaves SRAM, whose callbacks remain unresolved, or whose fixed backing and stack exceed composition's measured budget |

Run `cargo test -p oer-esp32s31-ieee80211-form` and `cargo xtask check ui`.
Generated ELF files and closure reports remain under ignored
`target/agent-a/form-placement/`.
