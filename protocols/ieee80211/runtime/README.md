# Wi-Fi runtime

`oer-ieee80211-runtime`: the asynchronous glue between the lower-MAC port
and the sans-IO cores. It selects over port events, `oer-time::Timer`
deadlines and application commands, hands each input to the cores and
carries out their actions through the port. It decides no protocol question
and neither parses nor builds frames: its normal dependencies are the port,
the cores (`sta`, `upper-mac`, `arbiter`) and portable support, never
`oer-ieee80211-wire`, `oer-ieee80211-cipher`, a chip crate, an executor or
Embassy time (`cargo test -p oer-test-layers`).

The station's key-data unwrap runs in a `KeyUnwrapSlot`: the image's
`oer_crypto::KeyUnwrap` engine as a static type, lent to `run` beside the
timer. The slot is one more source of the common select, beside port events,
commands and deadlines; its response returns the station's ticket before any
other input can end the phase. The engine's operation is `Unpin`. The caller
owns the slot, so an operation outlives a dropped run and keeps its buffer
until the engine finishes; the next run finishes it before serving, and a run
that ends finishes its own operation before the station retires. The runtime
depends on `oer-crypto`, not on the software engine in `cipher`.

`station` provides `Control` and the port-generic `Runner`; `monitor` holds the capture and
injection handoffs of a monitor. The frames a monitor keeps and the channels
it visits are `oer-ieee80211-upper-mac`'s `monitor`; the requested owner
topology is checked by `oer-ieee80211-arbiter`'s `configuration`.

`station::Control` owns the sole command producer, product status and a
cooperative stop connection. Control and its neutral network end can exist
before installation: connect, disconnect and scan submit only a kind and
correlation. Runner receives the original station command credits during
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

Host tests use the allocating in-memory backend from `oer-ieee80211-model`
as a dev-dependency. The model's fixtures run the shared conformance cases in
`oer-ieee80211-port`; runtime tests cover its station service over that model.

`StationPort` is a composition-owned mapping with a concrete `Backend`,
`SlotId` and `Caps`. `station::Runner` owns that backend's lifecycle directly.
This permits a composition to select station buffers and resource types without
implementing a foreign trait for a foreign backend or replacing its body-loan
identity. The radio and lower-MAC port contracts remain the common boundary.

`station::Runner` retains the station's concrete `StaCallbacks` provider with
its resource bank across stop/restart. It keeps no counters: it takes an
`O: oer_trace::Observer` (by default `Noop`, the observer of its station and
data machine too) and reports lost port events, receive drops with their
reason, MSDU refusals, unsent frames, group key installations and the ARP
correlation of the protected link as events of
`oer-ieee80211-instrumentation`. `ServiceOutput` carries only what the
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
sample reaches `RateSelection::learn` with its frame class.

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
`oer-time` timer and command-credit admission. Policy requests use correlations
1 (connect) and 65535 (graceful disconnect); explicit callers retain their
correlations. `CommandStarted` precedes the core call after credit admission.
Explicit Disconnect pauses the policy and Connect resumes it. Graceful Stop
has its own wake, admits one disconnect and retires after link-down. The raw
runtime `Control::stop` still requests immediate cooperative retirement.
Policy deadline exhaustion is a typed service failure. No policy timer or
initial automatic request exists when reconnect is disabled.
