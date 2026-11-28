# ESP32-S31 PHY architecture

This crate owns S31 RF algorithms, calibration state and finite hardware
transitions. It consumes typed HAL operations; it does not link the vendor PHY
library. Protocol runtimes own their MAC, descriptors, interrupt routes and
connection state. In the built tree a probe holds the shared radio domain
and calls the PHY's lifecycle, retune and tracking itself; the radio system,
which owned the domain and serialized those calls, is a source outside the
build.

The [chip overview](../../README.md) links protocol and coexistence scopes.
The [tracking contract](src/tracking/README.md) is the
canonical reference for demand, admission, cancellation and restoration.

The opt-in `lifecycle-fault-injection` feature exposes destructive one-shot
checkpoints in real maintenance. It owns no timer, reset mechanism or transport;
the caller must provide independent termination. Default builds omit its code.

## Terminology and phases

| Term | Meaning in this module |
| --- | --- |
| Initial calibration | Measurements and hardware setup performed during cold registration, before a protocol receives its operational radio owner. |
| Calibration results | Retained measurements and derived configuration. Having these values does not prove that hardware currently contains them. |
| Restore / replay | Re-establishing hardware from retained results after an invalidating power/reset transition. It is distinct from measuring new results. |
| Runtime tracking | The service of observing operating conditions, deciding which RF parameters need attention and executing admitted updates. It is an umbrella term, not a PLL-only operation. |
| Compensation | Computing and publishing an adjustment from an existing calibration model, such as temperature-dependent power/gain or analog-I2C band selection. It can still change hardware used by active traffic. |
| Recalibration | Repeating selected measurements or searches, such as common DCODE/RX gain or class TXDC/PWDET calibration. It does not necessarily repeat the full cold graph. |
| Maintenance | The execution interval and ownership transaction around tracking or another disruptive PHY operation. Demand for maintenance is not permission to execute it. |
| Packet reception adaptation | Per-frame AGC, carrier/frequency correction and channel estimation in the receive path. These are not the runtime temperature-tracking scheduler. |

RFPLL capacitor correction is named by its actual operation; it must not be
confused with complete frequency initialization or a proof of PLL lock.
Likewise, restoring MAC baseband enable is not restoring a protocol's DMA,
interrupt or connection epoch.

```mermaid
flowchart TD
    Power[Unique powered radio] --> Init[Cold registration and initial calibration]
    Init --> Registered[Registered PHY state and physical owner]
    Registered --> Role[Protocol operation]
    Role --> Demand[Read-only tracking demand]
    Demand --> Admission[Composition admits maintenance]
    Admission --> Track[Selected compensation or recalibration]
    Track --> Restore[Restore protocol hardware and checked release]
    Restore --> Role
    Init --> Cache[Optional calibration snapshot]
    Cache --> Validate[Validate schema, identity and complete products]
    Validate --> Init
    Track --> Fault[Failed epoch retained; reset required]
    Restore --> Fault
```

[Registration](src/calibration/registration.rs) admits a cache only when its
schema, physical identity, completion guards and RX table shape match. A valid
snapshot restores semantic calibration products and selects partial cold
calibration. The ordinary RF/baseband graph regenerates frequency memory and
republishes RX and TX gain state that cannot be assumed to survive a cold
epoch. An invalid or incomplete snapshot selects `FullAfterRejectedCache`.
Persistence belongs to the caller. Runtime tracking is not a substitute for
reset or retained-sleep wakeup replay.

## Owners and dependencies

| Owner | Responsibility | Boundary |
| --- | --- | --- |
| [Calibration](src/calibration/registration.rs), [analog](src/analog.rs), [RX](src/rx.rs), [TX](src/tx.rs) | Finite measurements, searches, ordered hardware actions and their results | No network sockets or protocol scheduling policy |
| [PHY state](src/state.rs) | Configuration, calibration results, temperature references and cache values | Values do not grant register access |
| [The one owner](src/owner.rs) and its [target operations](src/target_port/owner.rs) | The state's place, the registered handle, the proofs of RF open and closed, and when a tracking pass is due | The registered state has its own type; RF open and closed are proofs the operations take and give |
| [Tracking graphs](src/tracking.rs) and [executor](src/executor.rs) | Execute selected children and validate their completions | No independent RF arbitration |
| [Target port](src/target_port.rs), [family PHY](../../../family/radio/phy/README.md) and [HAL PHY](../../hal/src/radio/phy.rs) | Driver sequences over owned HAL register atoms | Hardware access is borrowed from the admitted owner |
| [Analog register bus](../../../family/drivers/regi2c/README.md) | Field, configuration and parallel transitions of the vendor analog-I2C leaves | The transitions touch no bus; the target port and HAL drive them |
| [Hardware coex control](../../../family/radio/coex/README.md) | Recovered timer requests, PTI, clock conversion and withdrawal accounting | Programmed timer identity is not an RF grant |

```mermaid
flowchart TD
    W[Its owner: the PHY's place and the radio domain] --> L[Radio domain]
    T[The owner's tracking timer] --> L
    P[tracking_due over the PHY state] --> T
    L --> E[PHY finite executor]
    E --> H[HAL / semantic PAC]
    H --> RF[Shared RF / PLL / analog buses / baseband]
    E --> G[Grant-protect request]
    G --> C[Hardware coex timer bank]
```

PHY supplies the algorithms and the state; its one owner runs registration,
close, wake, the channel and tracking; coex supplies the hardware arbitration
mechanism. Exclusive ownership of the PHY transaction serializes its register
access. Tracking brackets RF-sensitive regions with a grant-protect request
instead of pausing protocols. A coex timer reply or PTI is never an ownership
proof for the shared PHY, and the request bracket does not prove exclusive RF
access.

The Wi-Fi airtime scheduler selects packet service within a MAC epoch. It does
not own the shared RF resource and cannot independently admit PHY maintenance.

## Power lifecycle boundary

RF's power follows the owner's proofs: `RfOpen` while RF is open, `RfClosed`
while it is closed and the registration stays.

```mermaid
stateDiagram-v2
    [*] --> Open: register (the place filled)
    Open --> Closed: close (RfOpen taken, RfClosed given)
    Closed --> Open: wake (RfClosed taken, RfOpen given)
    Open --> Empty: a failed operation, then vacate
    Closed --> Empty: a failed wake, then vacate
    Empty --> Empty: register ends in RegistrationCause::Clock
```

After a failed close, wake, channel or pass the PHY's clock modules stay
held as the failure left them: a registration into the emptied place finds
them enabled (`AlreadyEnabled`) and ends in `RegistrationCause::Clock`.

RF close runs the current-vendor pre-close temperature observation and the
complete finite `phy_close_rf` graph: it disables hardware frequency control
and AGC, closes the RF and frontend/baseband domains
and publishes both retained analog close images. A preparation failure keeps
RF open and gives `RfOpen` back; a failure after close begins consumes the
proof.

Retained wake executes the complete current-vendor wake parent in recovered
order without retiring the registration epoch: frontend/baseband and
analog-I2C power, retained frequency publication, channel and TX-cap state,
PBUS boundaries, PHY and AGC register updates, CKGEN reset, then restoration
of hardware frequency control, BBPLL, force-TX/RX and baseband mode. Every
started failure consumes the proof.


### The one owner

The PHY's state has one owner, which holds its place (`owner::PhyPlace`):
the one right the radio's split hands out (`PhyStateRight`, which the
reunion takes back) and the owner's memory for the state. The radio domain beside it,
`RadioDomain<B>`, holds the shared registers and the power and clock
obligations; it carries no PHY state. `RadioDomain::detach(operation)`
produces one owned `RadioTransaction`; its explicit reunion preserves
registration epochs and pending restore obligations. The transaction can
split into an inert return owner and a movable `PhyTransaction` carrying the
hardware and grant-protect state. A split reunion rejects another domain's
transaction without consuming either payload. The domain identity is affine
and permanently claimed: equal registration epochs do not authorize a
foreign return. Successful reunion consumes both halves, so a stale return
token cannot remain live.

The operations (`target_port::owner`) take the domain by value and give it
back with their typed result:

- `register` takes the place, fills it with `PhyState::new` once
  (`Occupied::fill`) and lends the state to the registration's transitions.
  As ESP-IDF's calibrating first `esp_phy_enable`, it enables the `PHY` and
  `PHY_CALIBRATION` modem clock modules and releases `PHY_CALIBRATION`
  after calibration. Its result is `Registered`: `RegisteredPhy`, the
  handle of the filled place, and `RfOpen`. A failed registration empties
  the place and returns it with its leaf cause;
- `close` takes `RfOpen` and gives `RfClosed`: `phy_close_rf` runs, the
  temperature sensor powers down and `PHY` is released, as the last
  `esp_phy_disable` does. The registration stays;
- `wake` takes `RfClosed` and gives `RfOpen`: `PHY` and `PHY_CALIBRATION`
  are taken again, `phy_wakeup_init` restores the retained registration
  without calibration and `PHY_CALIBRATION` is released;
- `select_channel` and `switch_channel` take `RfOpen` and give it back,
  through the HAL Wi-Fi client's channel HAL; the switch stops and restarts
  the Wi-Fi MAC around the retune;
- `track` takes `RfOpen` and gives it back: one pass of
  `phy_param_track_tot`, Wi-Fi's only. As the vendor's periodic timer, it
  runs under the PHY lock alone (the owned transaction) and brackets its
  RF-sensitive regions with the grant-protect request. The pass's instant
  is recorded in the state;
- `enable_wifi_rx` and `disable_wifi_rx` need `RfOpen`.

Neither proof is `Copy` nor `Clone`, and only the operations' ends make them.
A failed operation returns its leaf cause and has consumed its proof; its
value carries `Vacate`, the one right `RegisteredPhy::vacate` takes, and the
place is empty again and nothing more: the clock modules stay held, so a
registration into it ends in `RegistrationCause::Clock`. A registered place
has no way back but a failure; an empty place gives its right back for the
reunion (`PhyPlace::into_right`). A registration that does not complete
releases both modules (`RegistrationCause::Failed { clocks, .. }`) and
gives its place back empty.

`owner::tracking_due(&PhyState, now)` answers whether a pass is due: at once
after a registration, and otherwise once more than `TRACKING_PERIOD`
(1 s, ESP-IDF's `PHY_TRACK_PLL_PERIOD_IN_US`) has passed since the last
pass. The owner asks after every registration and every wake, as the vendor
asks at every enable, and keeps its own timer for the next instant.

Registration, RF wake and RF close construct their async state directly in
their enclosing place and use `executor::boundary::Boundary`. Owned
lifecycle futures pin their borrowed PHY children in their retained state
and await pinned references. Completed child results remain in placed
fields of that retained state; only their exclusive `Placed` loans cross
the poll boundary.

Dropping either half of a transaction, or a pending lifecycle or tracking
future even before its first poll, quarantines all payloads without
dropping their owners or restoring registers. Its named fault retains the
cancelled operation and registration observation. Firmware binds the fault
directly at link time through the HAL's `phy_fault_reporter!` macro to the
chip composition's `phy_fault::reset` (`oer-chips-esp32s31-platform-runtime`),
which calls `fatal::reset_recording`; a 16-byte record naming the operation
and epoch is printed on the next boot and cleared only after successful
output. Host tests use an independent seat and catch their own
never-returning reporter.

## Conditions and cadence

The tracking pass reports what it committed (`PhyParamTrackingOutcome`).
Runtime sensor acquisitions have monotonic start/end provenance; undated
replacements invalidate that provenance. A due pass is not an instruction to
perform all heavy calibration: each child decides from its own references.
No hourly/daily calibration schedule or qualified maximum thermal deferral
is implemented.

## Runtime sequence and timing

A completed common RX calibration carries
[`PhyRxGainDcQuality`](src/rx/gain_calibration/quality.rs) alongside its
coefficients. It identifies convergence for every shared/Wi-Fi baseband gain
and the five `phy_rxdc_fine_cal` radio searches after Wi-Fi gain zero.
Exhausting a baseband search retains its initial pair; fine searches retain
their last correction. These vendor-compatible outcomes complete the transaction,
but do not mean every search converged. Completion flags describe lifecycle;
quality remains a separate result. A skipped DC branch has no new quality.

The source-derived tracking graph contains optional RFPLL work, Wi-Fi's
analog-I2C, power and calibration, followed by temperature sampling. Decisions in this graph use retained temperature;
the final sample must not be interpreted as the input to every preceding
decision. Temperature sampling can itself adjust the sensor's analog range.

`tracking_due` changes nothing while it answers. A pass records its instant
in the state when it ends; a pass that does not end records none, so the
next answer is still due. The period is the vendor's source interval, not a
measured maximum thermally safe deferral.

Nothing calibrates before the owner's `register`, which runs
[`run_phy_register`](src/executor/run.rs) over the state it lent: the cold
graph or its admitted cache path. A wake after RF close restores the
retained registration without calibration. `EmbassyClock` uses the
executor's SYSTIMER timer service: Alarm0 at P3 acknowledges and pends P2,
which dispatches timers and polls tasks. The direct maintenance TXDC/PWDET
path has a separate SAR/PWDET timer handler at P2.

The parameter-tracking graph selects individual operations internally; no
protocol selects them as independent maintenance transactions. Active-traffic
publication of individual compensation classes is not a qualified execution
mode.

The RX-gain path retains one [`RxProgram`](src/rx/program.rs) in both cold
registration and common tracking. Borrowed typed step tokens carry each
hardware command directly to one poll loop; consuming a completion updates
that same cursor and coefficient bank. Short ready leaves share a bounded
poll, while an incomplete DC/IQ observation waits on the caller's timer.
Other required 1–20 us hardware settles are `oer_portable_settle` holds on
the settle counter the caller gives beside its timer (`executor::wait::PhyTimer`):
on the chip, the HAL's `CycleCounter` at the CPU rate the runtime handover adopted.
A timer made with `PhyTimer::counted` tells its tally (`executor::wait::WaitTally`)
of every wait as it ends, by kind (`Waited`: a spun or a timed settle, a
completion or bus-busy wait, a step engine's retry, a cooperative yield),
with the microseconds asked and taken; `PhyTimer::new`'s tally counts nothing
and reads no clock for it.
PBus/I2C leaves retain their bounded direct observation policy.
TXDC/PWDET retains its direct search/root interpreter inside the owned PHY
transaction, without CPU-admission queries between its direct operations. Each runtime SAR/PWDET conversion uses the explicit [step machine](src/tx/sar.rs)
and [affine timer service](src/sar_pilot.rs), with no future polled for its
hardware progress. The actor submits `PhyTransaction`, TIMG0's program owner,
the binding proof and machine through a one-slot queue. `DetachedRadioDomain`
stays in the waiting future, retaining cancellation and clock obligations;
calibration state is outside the transferred hardware half. Between events
the handler retains the active bundle in its own cell. Completion stops the
alarm and creates a distinct `CompletedBundle` with a mandatory result, returning it
through a one-slot queue and a same-level `IrqSignal<InterruptExecutor<Esp32s31, 2>>`
declared with `FromExecutorLevel`. The leaf still suspends asynchronously at
its submission boundary: `Client::sample` has one `poll_fn` to validate/register
the executor context and one to wait for the reply. The async leaf-step count
is unchanged: the reply wait replaces the readiness-timer wait.
The table permanently binds TIMG0's interrupt owner as `Exempt(ExecutorLevel)`.
Startup masks and CLR-clears old RAW; the handler acknowledges each event
before rearming. Rearm does not acknowledge old pending. The measured S31 P2
pair assigns FROM_CPU2 line 17 ahead of TIMG0 line 16; other pairs/levels are
unqualified by that measurement. P1 entries use 40–42, retaining their order.
Each event performs one bounded I/O atom. The vendor's tone/trigger minima
are 1/2 µs. The machine returns those fixed relative intervals after tone arm
and SAR trigger; the handler reloads them after the edge without subtracting a
second clock reading. Crossing a microsecond boundary between the edge and
rearm cannot shorten either interval. Handler lateness and programming overhead
lengthen the wait; no upper latency bound is claimed.
An acknowledged event without a request or active bundle returns
`Err(SarAlarmWithoutBundle)` with the sampled RAW/ST and both queue lengths.
All owners remain held; the image immediately retains the reason and resets
through its existing direct call, without further service. Cancelling
the waiting lifecycle future records its admitted operation; an in-flight
alarm remains quarantined until reset, while completion already stopped/masked
it before publishing the reply.
Readiness retries rearm TIMG0 for 1 µs under our 40 µs engineering limit;
the ROM reference supplies no readiness deadline. Other PHY children,
including cold registration's existing graph bindings, remain async. The
handler does not call cooperative admission or a task waker. The current P2
executor round and P3 work can delay it; continuous higher-line P2 work can
starve it. RF quality, SRAM/stack cost and service latency for this pilot are
unqualified; the virtual-time tests establish ordering and minimum waits. A sample that reads ready
succeeds at any time, even after a late wake-up; the deadline bounds only the
not-ready case.
Interrupts remain enabled and the physical owner remains held until terminal
restoration. Longer registration, tracking and lifecycle operations wait on
the caller's `oer_portable_clock::Timer` through `executor::wait::delay` and its
`executor::wait::Kind`.

Direct PBus, analog-I2C and TX-DC/PWDET status observations use a named
20 µs elapsed-time budget on the caller's clock, including a final observation
after the limit is seen. This is a readiness policy, not a minimum delay or a
20 µs bound on a whole transaction. Timeout remains a typed failure.
The retained-wake frequency-ready edge already belongs to an asynchronous
transaction and retries through the timer in 10 µs steps.

The final temperature child of parameter tracking uses the private
[`executor::step`](src/executor/step.rs) transport driver. Its read, masked
read/modify/write and sensor-code atoms share the driver's cooperative batching.
The first bus completion sample follows a 1 µs timer backoff; not-ready retries
use the caller's timer. This retry cadence is separate from a vendor minimum
settle. T7's named answer window is our 100 µs budget from completed command
publication. The bus is sampled before judging the deadline: a completed
answer is accepted even after a late resume; only a still-busy bus times out.
The existing temperature graph owns conversion, range writes and dated commit.
Cancellation quarantines the owned transaction until reset.
At `Runnable`, the driver continues while CPU admission allows; otherwise it
self-wakes and returns `Pending` without a timer. A future `WaitAt` constructs
and polls the timer; an already reached `WaitAt` still yields once before
another hardware step. The gap between RMW commands has no command in
flight; its first busy-at-start attempt establishes that phase's bound.

Wi-Fi temperature-band tracking (T3) uses the same P2 driver and counted I2C
cursor as cold RFPLL. The two masked writes retain their order, sampled bytes
and exclusive analog owner across `Pending`; only completion of both writes
allows the retained band to commit. These counted commands keep their original
`HARDWARE_EDGE_LIMIT` and first/repeated 1 µs observations. Their scheduler
pauses are not charged against a direct-path elapsed settle. The diagnostic
hold reports T3's engine polls as family 3 after the tracking root returns.

Diagnostics retain bounded timing and command observations and report only
after the root returns. Overflow or incomplete event pairing invalidates the
report; observer time and interrupts remain included.

CPU admission is checked only in the [shared transport](src/executor/step.rs)
and [RX-gain](src/executor/rx.rs) drivers, between complete atoms. The
[executor's `should_yield`](../../../family/platform/executor/src/executor.rs) accounts
for ready tasks, the executor's pending software interrupt, the radio doorbell,
unserviced typed signals and the current task poll's elapsed time. The drivers also stop admitting atoms
when their local batch reaches 900 µs. Per-command RFPLL tracing yields at each
checkpoint. The 50 µs atom and 1 ms poll limits are cooperative contracts:
there is no timer that interrupts an overlong atom or enforces them throughout
every parent future. Direct TXDC/PWDET and the cold registration parent have
no admission checks of their own; immediately ready children can chain in one
poll. The executor round can include several task polls.

The timing mechanisms have different scopes:

| Mechanism | Meaning | Enforcement / limitation |
| --- | --- | --- |
| Vendor minimum settle | Wait at least the required time after a hardware action | Named 1–20 µs settles count cycles on the counter beside the caller's timer; longer settles use the timer. Preemption can lengthen them. |
| Readiness deadline | Our bound for observing completion or giving up on busy hardware | Direct observations use 20 µs; T7 accepts a completed answer after its 100 µs busy window; SAR gives up after 40 µs without a ready sample and accepts any ready sample. These are not vendor maximum delays. |
| CPU admission | Decide whether another complete atom may run | Stop at a ready peer or 900 µs admission threshold, reserving 100 µs toward the 1 ms contract. No universal atom/poll elapsed-time cutoff. |
| RF admission | Decide whether the shared RF transaction may start | Pending tracking state and the owned PHY transaction; grant-protect brackets request hardware arbitration. No hard-window calendar, grant proof or one-period deferral bound. |

Finite execution limits are separate from those timing mechanisms:

| Mechanism | Established bound | Limitation |
| --- | --- | --- |
| [Direct bus polling](src/executor/wait/poll.rs) and [target executor](src/target_executor.rs) | Finite observations per PBus/I2C operation; typed failure on exhaustion | Requires each MMIO access to return; nested operations have separate observation budgets |
| [DC/IQ estimator](src/calibration/estimator.rs) | Finite readiness samples, then measurement/start cleanup on missing-ready | Cleanup and settle calls must themselves return |
| [Calibration executor](src/target_port/calibration.rs) | Root operation accounting and finite gain-state loops | Operation counts do not bound IRQ interference or a stalled bus access |
| [RFPLL search](src/tracking/rfpll/search.rs) | Both search directions terminate after finite samples, including unrecognized status values | Its finite state graph still invokes synchronous bus and settle operations |

The BLE radio owner arms the independent SoC deadline service around
physical lifecycle transactions and keeps the radio domain until checked restoration.
The board/application supplies explicit engineering budgets; PHY owns no timer
or reset peripheral. Hardware reset does not by itself establish a qualified
RF-cessation bound. A pending-future timeout, observed calibration maximum and
finite loop count cannot establish that stronger guarantee either. Failed or
expired work cannot manufacture runnable ownership.

Timing qualification must cover the complete action-to-hardware path, including
executor-added waits, readiness retries, failure cleanup and the selected
runtime adapter. Every wait of at most 100 us requires hot-path classification,
with waits of at most 20 us reviewed first. Record the required minimum,
first-read placement, retry predicate and bound, implementation mechanism, and
measured elapsed time separately. Include variable-duration waits whose range
can enter this interval. A compiled semantic comparison alone does not qualify
these timing properties. Vendor references must include the linked firmware
and its ROM callees: ESP-IDF can override archive leaves, including the I2C
critical-section hooks. Code-placement comparisons require an aligned wait
baseline first.

CPU waiting time, RF exclusion and unavailable packet service are different
quantities. Async hardware waits can release the CPU while the radio remains
unavailable. The current executor uses bounded hardware observations where
there is no owned completion interrupt. Moving those observations to a timer
does not make a long calibration harmless to traffic.

RX-gain host oracles exercise the same program and pure search arithmetic as
production. The frozen leaf/outcome projection exists only in tests;
hardware vendor comparison runs the real program through the validation owner.
There is no per-leaf child future or nested RX action/binding/completion path.
Every DC/IQ estimator
consumes the same root operation budget, while the fixed outer calibration
graph has its own finite structural bounds. Budget exhaustion cannot produce a
complete coefficient set, and failure retains the recovered cleanup and
outer-control restoration sequence.

The fixed bank encodings are generated at compile time and shared as read-only
data. Measured DC coefficients remain in the individual transition, and entry
encoding borrows them. Terminal products are inspected separately from hardware
admission. Compile-time budgets cover both retained transitions and the compact
external bindings that cross executor call boundaries.

RX diagnostics record only the root interval, its single-poll execution, three
disjoint direct phases and clock-free path counts. They do not install callbacks
inside the minimum-search loop. Search traces and elapsed hardware timing
remain separate measurement scopes.

Observed idle tracking maxima do not establish RF exclusion, packet delivery
or a worst-case hold bound. Grant-protect intervals time the request, not an
independently observed grant.

## Restoration and failure

The physical operation owns register access across all nested waits. The
protocol retains packet storage, DMA leases and its paused runtime. MAC or
baseband writes inside calibration cannot manufacture a resumable protocol
epoch. Release checks and the exact RX/IRQ handoff establish that boundary.

A pending demand wait is cancellable without starting hardware work. An
abandoned consuming execution quarantines its owners and records a named
fault before reset. A failed execution returns its typed failure and has
consumed its proof, without automatic rollback. Restoration failure also requires reset. Holding
a CPU critical section across async waits is not the exclusion model.

Calibration completion, restoration completion and measured RF quality are
distinct outcomes. The existing branch flags establish the first, while
protocol restoration establishes the second. Neither proves EVM, calibrated
radiated power, sensitivity or sustained thermal stability.

## Source and validation boundary

The pinned `libphy` in [vendor artifacts](../../vendor/artifacts.toml) supplies
the recovered parent order and child algorithms. Host and compiled vendor
comparisons check semantic state and ordered effects under modeled inputs;
they do not establish RF quality, physical exclusion or elapsed hardware timing.
[Tracking](src/tracking/README.md) and [RFPLL](src/tracking/rfpll/README.md)
describe the production algorithms and their terminal obligations.
