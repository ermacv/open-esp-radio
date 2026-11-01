# Wi-Fi runtime

`oer-ieee80211-runtime`: the asynchronous glue between the lower-MAC port
and the sans-IO cores. It selects over port events, `oer-time::Timer`
deadlines and application commands, hands each input to the cores and
carries out their actions through the port. It decides no protocol question
and neither parses nor builds frames: its normal dependencies are the port,
the cores (`sta`, `upper-mac`, `arbiter`) and portable support, never
`oer-ieee80211-wire`, `oer-ieee80211-core-security-cipher`, a chip crate, an executor or
Embassy time (`cargo test -p oer-test-layers`).

The station unwraps its key data in its own step, over the block
primitive its callbacks supply; the runtime carries no unwrap.

`station` provides `Control` and the port-generic `Runner`; `monitor` holds the capture and
injection handoffs of a monitor. The frames a monitor keeps and the channels
it visits are `oer-ieee80211-core-upper-mac`'s `monitor`; the requested owner
topology is checked by `oer-ieee80211-core-arbiter`'s `configuration`.

`station::Control` owns the sole command producer, product status and a
cooperative stop connection. Control and its neutral network end can exist
before installation: connect, disconnect and scan submit only a kind and an
internal correlation, and return an `Operation` that resolves exactly once to
that request's `OperationEnd`. `connect(profile)` also names the network: the
`StaProfile` (SSID and security) is published to `State` in the same poll as
its request, so a connect cancelled while it waits for queue room leaves no
profile behind, and Runner takes it with that request. Runner gives it to the
core station before the Connect; a station that is not idle refuses it and
the Connect then gets the same refusal (`AlreadyConnected` or
`Transitioning`). A refused, stopped or cancelled connect drops its profile,
whose secrets erase themselves; a service end or a new control epoch drops
an unserved one. The station keeps the profile it took for later
connections; a station that never took one refuses Connect with `NoProfile`. Each kind has one permission: an admitted
request holds it until its terminal, so a second request of the kind waits
for it instead of posting a duplicate. Dropping the operation detaches its
observer without returning the permission; the late terminal returns it and
never reaches the next request. A service end resolves waiting operations
with `ServiceEnded`. Runner receives the original station command credits during
construction. It retains them through core refusal/completion and retirement,
restores them before delivering a concrete callback, and reports a duplicate
in-flight kind without minting a permission. No installation lifetime or
pool path crosses the application command boundary.

`State` retains status and the `oer-time` connected-task connection. `Control::stop`
requests cooperative Runner shutdown independently of command-queue capacity.
Runner completion is published only after machine retirement; dropping its
endpoint poisons reuse. The completed radio/resource banks remain in the
Runner's owned result. `Control::wait_stopped` observes completion;
`try_finish` consumes the control epoch and returns its command producer.
The `connected-tasks` feature is enabled by the runtime, never by a core.

Host tests use the allocating in-memory backend from `oer-ieee80211-testing-model`
as a dev-dependency. The model's fixtures run the shared conformance cases in
`oer-ieee80211-contracts-port`; runtime tests cover its station service over that model.

`StationPort` is a composition-owned mapping with a concrete `Backend`,
`SlotId` and `Caps`. `station::Runner` owns that backend's lifecycle directly.
This permits a composition to select station buffers and resource types without
implementing a foreign trait for a foreign backend or replacing its body-loan
identity. The radio and lower-MAC port contracts remain the common boundary.
The service reports the start and end of each stretch of its own work
(`ServiceSpan`: a step, the pending frames, a sources poll, a received frame,
its delivery and network hand-off, a terminal, a transmit, a recycle) to
`StationPort::span`, which by default observes nothing.

`station::Runner` retains the station's concrete `StaCallbacks` provider with
its resource bank across stop/restart. It keeps no counters: it holds the
composition's `O: StationEvents` sink (by default `()`), lends it to each call
of its station, data machine and receive agreements, and reports lost port
events, receive drops with their reason, the MSDUs of a received frame the
network queue did not take (one `RxMsdusRefused` count per reason and frame,
after the station's own events of that frame) and group key installations as
`ServiceEvent`s. A refusal it returns to the application (`MsduRefused`,
`TxRefused`) is the application's to count and is no event. Its types carry no transport
encoding. `ServiceOutput` carries only what the
application acts on: notices, command terminals, refusals that return an
owner, and the arbiter's access-point commands. `ServiceIo` borrows a concrete
`ServiceOutputSink`; closures implement that trait without erasing their type.
Frame refusals transfer their original frame owner by value. Command events carry
the original kind and correlation; the affine permission stays with Runner.
Commands and network frames travel through `oer_memory::WakingSpscQueue`.

Every transmission's parameters come from the cores: a station frame's rate
plan, coexistence class, access category, protection exchange and power from
`sta`'s action, its CCMP packet number from the station's pairwise key, and a
data MPDU's from `sta::data`. The service prepares the port's source in the
buffer pool, submits it and routes each terminal back; a terminal's rate
sample reaches `RateSelection::learn` with its frame class. The station's
PTK installation is pending until its `PairwiseKeyInstalled` terminal: only
then does `sta` receive `PairwiseKeyInstalled` with the key numbered from the
request's packet-number step.

The image lends `station::Runner` a placed `RxBlockAckStorage`, the upper
MAC's receive Block Ack owner bound to this port's RX owners, bank slots and
the station's link epoch. Its const generic bank count selects existing shared
RX BA slot owners; retained frames borrow the port's RX buffers without
copying payloads. The owner decides every agreement edge; the service hands
it the station's validated Block Ack actions and received frames, executes
its bank and frame transitions on the phase (`RxBlockAckPort`), delivers the
frames it releases in order and sends the replies it owes through the
station's control credits. The image supplies the reorder gap duration.

The monitor handoffs take no lock. A capture moves a frame copied into the
caller's pool through an SPSC queue; an injection request moves its bytes by
value through one, and its completion back through another. The active
dwell's binding sits in a `ClaimCell`: a side that finds it lent to the other
at that instant is refused with a typed result (`DwellChanging`,
`BindingBusy`) and retries.

## Known defects

- **`station.rs::serve` steps once per loop**, then handles the
  event, tick, command and transmit in one pass; a later input sees the
  machine before the previous one was stepped.
- **Queue bounds are not tied to the backend.** `sta`'s `StaData::new`
  takes a caller `depth` and `ArbiterPolicy::checked` a caller
  `publication_bound`; neither comes from `TX_QUEUE_DEPTH` /
  `PUBLICATION_BOUND` of the lower-MAC owner, so they can disagree.
- **No bound on a gate cycle or a drain.** `drain_port_error` has no
  deadline for a gate that never closes, and `MAX_INPUT_STEPS_PER_BATCH`
  only yields instead of raising a contract fault.
- **Low:** retired-service fields `_pending`, `_arbiter` and `_places` remain
  in `station.rs`.

Control's public records belong to the runtime; their semantic payloads live
in `station::contract`, without wire serialization or re-exports of
station/upper-MAC core types. Core conversions are exhaustive inside runtime;
refused Ethernet owners stay opaque and affine. `Device::new` is the separate
network wiring constructor. The HIL binding translates only the public contract.

The station runtime owns its optional reconnect policy through the existing
`oer-time` timer. Station requests have one admission: each command kind's
permission in `State`, which an `Operation`'s request and the policy's
request both take and which returns at that request's terminal. While
another request holds it, `connect`/`disconnect`/`scan` wait and the policy
neither issues nor arms its deadline; no request is posted to be refused
for a held credit. `ControlCommand` carries only access-point facts and
Stop, so a control producer cannot post an unadmitted station request; a
station request without its credit is the `CommandAdmission` service
fault. Policy requests use correlations 1 (connect) and 65535 (graceful
disconnect); operations draw theirs from 2^32 upward. A new control epoch
frees every permission; an operation of the ended epoch reads its retired
terminal or `ServiceEnded`, and the epoch is refused with
`ControlCreateError::Unread` while an earlier epoch's retired result of the
same kind is still unread. `CommandStarted` precedes the core call after
credit admission.
The policy never connects first: it becomes active at the application's
first admitted connect, which named the network, and its reconnections use
the profile the station kept; they publish none.
Explicit Disconnect pauses the policy and Connect resumes it. Graceful Stop
has its own wake, admits one disconnect and retires after link-down. The raw
runtime `Control::stop` still requests immediate cooperative retirement.
Policy deadline exhaustion is a typed service failure. No policy timer
exists when reconnect is disabled.
