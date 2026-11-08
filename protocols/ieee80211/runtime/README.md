# Wi-Fi runtime

`oer-protocols-ieee80211-runtime`: the asynchronous glue between the lower-MAC port
and the sans-IO cores. It waits on port facts, `oer-portable-time::Timer`
deadlines and application commands, hands each input to the cores and
carries out their actions through the port. It decides no protocol question
and neither parses nor builds frames: its normal dependencies are the port,
the cores (`sta`, `upper-mac`, `arbiter`) and portable support, never
`oer-protocols-ieee80211-wire`, `oer-protocols-ieee80211-core-security-cipher`, a chip crate, an executor or
Embassy time (`cargo test -p oer-tools-test-layers`).

The station unwraps its key data in its own step, over the block
primitive its callbacks supply; the runtime carries no unwrap.

The station runs as two tasks on one executor. `datapath` is the station's
datapath actor over a `DataPort`: the one task that owns the radio, carries
the data both ways, numbers the link's frames, owns the receive agreements
and the rate selection, and runs the lifecycle's port commands. `lifecycle`
owns the station machine, its arbiter, the contexts of the station's frames
at their tickets' places and the station's group tokens; it never holds the
port and talks to the actor only in messages (`ToActor`, `FromActor`),
owners too large for them travel beside them (the return, parcel, group and
link queues). `station` holds the application's `Control` and the
vocabulary both tasks share: commands and outputs, the network's ends and
their wake topology, events, command credits, admissions, transmit context
places and the receive Block Ack storage. `monitor` holds the capture and
injection handoffs of a monitor.

Each actor turn reads the clock once and serves the transmit facts, the
lifecycle's messages, at most 32 received units, the network's MSDUs and
the freed memory; the actor wakes itself only when a budget ran out with
work left. While the message budget leaves messages queued, received units
wait for the next turn, so a link handed over before them is in force when
they are routed. A stop withdraws a command the port holds at its barrier:
it comes back refused `NotEnabled`, and the stopping lifecycle drops its
owners with the installation. The lifecycle asks the actor for the link
back once the station's leave frames numbered on the link went before it. Air input never ends it: only the port's named fault or the
actor's own broken contract does, with every owner kept. The lifecycle
wakes on the actor's messages, the application's commands and stop, and
one alarm at the station's, arbiter's and reconnect policy's deadline; at
steady transmission it wakes only on the actor's coalesced facts, never per
PPDU. The frames a monitor keeps and the channels it visits are
`oer-protocols-ieee80211-core-upper-mac`'s `monitor`; the requested owner
topology is checked by `oer-protocols-ieee80211-core-arbiter`'s
`configuration`.

`station::Control` owns the sole command producer, product status and a
cooperative stop connection. Control and its neutral network end can exist
before installation: connect, disconnect and scan submit only a kind and an
internal correlation, and return an `Operation` that resolves exactly once to
that request's `OperationEnd`. `connect(profile)` also names the network: the
`StaProfile` (SSID and security) is published to `State` in the same poll as
its request, so a connect cancelled while it waits for queue room leaves no
profile behind, and the lifecycle takes it with that request. The
lifecycle gives it to the core station before the Connect; a station that
is not idle refuses it and the Connect then gets the same refusal (`AlreadyConnected` or
`Transitioning`). A refused, stopped or cancelled connect drops its profile,
whose secrets erase themselves; a service end or a new control epoch drops
an unserved one. The station keeps the profile it took for later
connections; a station that never took one refuses Connect with `NoProfile`. Each kind has one permission: an admitted
request holds it until its terminal, so a second request of the kind waits
for it instead of posting a duplicate. Dropping the operation detaches its
observer without returning the permission; the late terminal returns it and
never reaches the next request. A service end resolves waiting operations
with `ServiceEnded`. The lifecycle receives the original station command
credits during construction. It retains them through core
refusal/completion and retirement, restores them before delivering a
concrete callback, and reports a duplicate in-flight kind without minting a
permission. No installation lifetime or
pool path crosses the application command boundary.

`State` retains status and the `oer-portable-time` connected-task connection. `Control::stop`
requests cooperative lifecycle shutdown independently of command-queue
capacity. Lifecycle completion is published only after machine retirement;
dropping its endpoint (`RunnerControl`) poisons reuse. `Control::wait_stopped`
observes completion; `try_finish` consumes the control epoch and returns its
command producer.
The `connected-tasks` feature is enabled by the runtime, never by a core.

Host tests run the lifecycle and the actor over the allocating in-memory
port of `oer-protocols-ieee80211-testing-model`, a dev-dependency; the
model's fixtures run the shared conformance cases in
`oer-protocols-ieee80211-contracts-port`.

The retired lifecycle returns the station's concrete `StaCallbacks` provider
with its resource bank (`RetiredLifecycle`). Neither task keeps counters: each holds
the composition's `O: StationEvents` sink (by default `()`), lends it to
each call of its station, data plane and receive agreements, and reports
receive drops with their reason, the MSDUs of a received frame the network
queue did not take (one `RxMsdusRefused` count per reason and frame, after
the station's own events of that frame), the MSDUs it copied instead of
lending and group key installations as `ServiceEvent`s. A refusal returned
to the application (`MsduRefused`, `TxRefused`) is the application's to
count and is no event. `ServiceOutput` carries only what the application
acts on: notices, command terminals, refusals that return an owner, and the
arbiter's access-point commands; `LifecycleIo` borrows a concrete
`ServiceOutputSink`, and closures implement that trait without erasing
their type. Command events carry the original kind and correlation; the
affine permission stays with the lifecycle.

The station's link goes to the actor at the station's handover at connect
and comes back at disconnect, before the station finishes. The actor
admits the link's received data itself: data MSDUs go to the network,
EAPOL to the lifecycle for `Station::eapol`, every other frame and every
frame before the link to the lifecycle for `Station::receive`, and back to
the ring once it is done. Data goes through the port's two data arenas,
filled and published by `sta`'s data plane in the actor; each arena's end
gives the network's memory back, owes a BlockAckReq to the station's
transmit agreement and teaches the rate selection. A group key's terminal
switches its replay lane in the link before the station takes it.

The image lends the actor a placed `RxBlockAckStorage`, the upper MAC's
receive Block Ack owner bound to the port's RX owners, bank slots and the
station's link epoch. Its const generic bank count selects existing shared
RX BA slot owners; retained frames borrow the port's RX buffers without
copying payloads. The owner decides every agreement edge; the actor hands
it the station's validated Block Ack actions and received frames, executes
its bank and frame transitions on the port (`RxBlockAckPort`), delivers the
frames it releases in order and sends the replies it owes to the lifecycle.
The image supplies the reorder gap duration.

The monitor handoffs take no lock. A capture moves a frame copied into the
caller's pool through an SPSC queue; an injection request moves its bytes by
value through one, and its completion back through another. The active
dwell's binding sits in a `ClaimCell`: a side that finds it lent to the other
at that instant is refused with a typed result (`DwellChanging`,
`BindingBusy`) and retries.

Control's public records belong to the runtime; their semantic payloads live
in `station::contract`, without wire serialization or re-exports of
station/upper-MAC core types. Core conversions are exhaustive inside runtime;
refused Ethernet owners stay opaque and affine. `Device::new` is the separate
network wiring constructor. The HIL binding translates only the public contract.

The station runtime owns its optional reconnect policy through the existing
`oer-portable-time` timer. Station requests have one admission: each command kind's
permission in `State`, which an `Operation`'s request and the policy's
request both take and which returns at that request's terminal. While
another request holds it, `connect`/`disconnect`/`scan` wait and the policy
neither issues nor arms its deadline; no request is posted to be refused
for a held credit. `ControlCommand` carries only access-point facts and
Stop, so a control producer cannot post an unadmitted station request; a
station request without its credit is the `CommandAdmission` station
failure. Policy requests use correlations 1 (connect) and 65535 (graceful
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
has its own wake, admits one disconnect and retires after link-down; a
station without a link refuses that disconnect and the lifecycle then takes
the stop. The lifecycle looks at the policy again after it registers for it,
so a policy change its own step made needs no later wake. The raw
runtime `Control::stop` still requests immediate cooperative retirement.

A stop that stops the network returns the network's transmit memory before
it asks the network to stop: once the radio stopped and its arenas settled,
the ended frames' memory and every MSDU left in the transmit queue (each as
data left behind) go back through the return queue, as its room allows, and
only then does the actor ask the network to stop. The network takes that
memory back before its port ends, so a confirmed stop leaves none of it with
the station. The radio's stop and the network's return are separate: a
network that has not returned its memory, its stop or its lent units by the
deadline ends the stop's wait there as `StopOutcome::Returning` (the
lifecycle's `LifecycleEnd::StoppedReturning`), never as an unconfirmed stop.
The return goes on without a deadline, the memory stays the station's until
the network took it, and a retirement ends the actor only once the return
came. A lost network participant (its stop never reported) is a software
error, not a fault of the radio: the station still returns its share of the
network through the return queue and waits for its lent units, and once
they are home the stop ends with `StopOutcome::NetworkLost`
(`LifecycleEnd::StoppedNetworkLost`) before the deadline, or the late return
completes after it; `Actor::network_lost` tells the composition, which
records it. Neither end is terminal. The deadline ends the lifecycle's wait;
the facade's public stop waits for the actual return and the radio's
release.
A stop for the next installation keeps the network and its MSDUs. An actor
reinstalled after a stop that stopped the network asks the next network to
stop again at its own stop. A new control epoch's reconnect policy keeps its
configuration and forgets the ended epoch's stop and network.
Policy deadline exhaustion is a typed station failure. No policy timer
exists when reconnect is disabled.

## Known defects

- **Publication bound not tied to the backend.** `ArbiterPolicy::checked`
  takes a caller `publication_bound`, not `PUBLICATION_BOUND` of the
  lower-MAC owner, so they can disagree.
