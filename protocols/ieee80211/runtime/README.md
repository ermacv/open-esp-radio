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

`station_service` is the station's service; `monitor` holds the capture and
injection handoffs of a monitor. The frames a monitor keeps and the channels
it visits are `oer-ieee80211-upper-mac`'s `monitor`; the requested owner
topology is checked by `oer-ieee80211-esp-now`'s `configuration`.

Host tests use the allocating in-memory backend from `oer-ieee80211-model`
as a dev-dependency. The model's fixtures run the shared conformance cases in
`oer-ieee80211-port`; runtime tests cover its station service over that model.

`StationPort` is an image-owned composition mapping with a concrete `Backend`,
`SlotId` and `Caps`. `StationService` owns that backend's lifecycle directly.
This permits a composition to select station buffers and resource types without
implementing a foreign trait for a foreign backend or replacing its body-loan
identity. The radio and lower-MAC port contracts remain the common boundary.

`StationService` retains the station's concrete `StaCallbacks` provider with
its resource bank across stop/restart. `ServiceIo` borrows a concrete
`ServiceOutputSink`; closures implement that trait without erasing their type.
Each output still transfers its original command or frame owner by value.
Commands and network frames travel through `oer_memory::WakingSpscQueue`.

Every transmission's parameters come from the cores: a station frame's rate
plan, coexistence class, access category, protection exchange and power from
`sta`'s action, its CCMP packet number from the station's pairwise key, and a
data MPDU's from `sta::data`. The service prepares the port's source in the
buffer pool, submits it and routes each terminal back; a terminal's rate
sample reaches `RateSelection::learn` with its frame class.

The image lends `StationService` a placed `RxBlockAckStorage`, the upper
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

- **`station_service.rs::serve` steps once per loop**, then handles the
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
  in `station_service.rs`.
