# Wi-Fi runtime

`oer-ieee80211-runtime`: portable Wi-Fi execution primitives — monitor
handoffs, task shutdown, station network ownership and poll boundaries.

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

The image lends `StationService` a placed `RxBlockAckStorage`. Its const generic
bank count selects existing shared RX BA slot owners; retained frames borrow
the port's RX buffers without copying payloads. Negotiation bounds each window
by the remaining frame credits and reserves one credit to close a reorder gap.
The service keeps pending responses while control TX is busy, reorders QoS data,
advances a window on a matching compressed BAR, and releases frames and bank
owners on peer DELBA, inactivity, disconnection or healthy Stop. ADDBA refusal
uses an explicit status when the parameters or available resources cannot admit
an agreement. The image supplies the reorder gap duration.

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
- **Low:** retired-service fields `_pending`, `_arbiter`, `_places` and the
  `MsduQueue` name remain in `station_service.rs`.
