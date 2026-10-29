# IEEE 802.11 observation events

`oer-ieee80211-instrumentation` is the event vocabulary of the IEEE 802.11
cores and runtime: a portable `no_std` crate without `unsafe` code or runtime
panics, built for the target and the host. It depends only on `oer-trace`;
no core, runtime, driver or HAL. It is instrumentation, not a product
interface or a control vocabulary.

## Events

Each event implements `oer_trace::Event` in the IEEE 802.11 domain with ids
below 100 (the ESP32-S31 driver's lower-MAC trace uses 100 and up) and one of
these channels:

| Channel | Owner | Events |
| --- | --- | --- |
| `STATION` | station core (`oer-ieee80211-sta`) | `TokenLost`, `DataMsduLost`, `RsnInputDropped`, `ScanBssObserved` |
| `SERVICE` | station service (`oer-ieee80211-runtime`) and station composition | `PortEventsLost`, `RxFrameDropped`, `RxMsduRefused`, `TxUnsent`, `TxMsduTooLong`, `GroupKeyInstalled`, station activity events (below) |
| `ARP` | station service | `ArpAddresses` followed by `ArpTransmitted` or `ArpReceived` |
| `ESP_NOW` | ESP-NOW encrypted peer table | `EspNowPeerTableChanged`, `EspNowPeersCleared`, `EspNowTxPacketNumber`, `EspNowReplay` |

An owner reports with `oer_trace::observe!` to its `O: Observer`, by default
`Noop`; without `cfg(oer_trace)` a point leaves no code. No owner keeps a
count of these events: a count is the number of events of a kind, which the
image's collector keeps. `ArpAddresses::parse` projects the RFC 826 fields of
an ARP MSDU inside the owner's observation point.

Station activity has independent kinds 22..=28 on `SERVICE`:
`StationAuthenticated`, `StationBeacon`, `StationAssociated`,
`TxIndeterminate`, `TxHardwareTimeout`, `TxQueueCollision` and
`ControlDelivered`. Each is part of `Observation` and
`Recorder`. `StationActivity::of` projects these events for image-local
correlation without depending on the facade. `TxIndeterminate` denotes every
unknown single-TX outcome; `TxHardwareTimeout` denotes only a timeout reported
to rate selection. `TxQueueCollision` denotes one sampled pending EDCA queue.
A beacon carries wrapping milliseconds from the image's monotonic clock.
Collectors retain these distinct meanings and own their counts. The generic capture service exposes a fixed bank of atomic kind counters, including supported zeros.

## Image adapters and test fixtures

- `Observation::of(&event)` turns any event an observer receives into one
  typed value of this vocabulary (`None` for other vocabularies), so a
  collector's `Observer::event` is a `match`.

- `Recorder<N>` (feature `test-support`) keeps the first `N` observations in
  order, for host tests of owners and collectors under `--cfg oer_trace`.

## Checks

Unit tests (`src/tests.rs`): every event decodes to itself through its words
and typed projections, kinds are unique and below the driver's range, channels
stay below the driver's, malformed words are refused, receive drop codes
follow the upper MAC's order, ARP parsing accepts only Ethernet/IPv4
requests and replies, and the recorder keeps observations only with
`cfg(oer_trace)`.

## Aggregation and rate selection

Kinds 36..=40 on `AGGREGATION` describe `RxContainment`, `RxReorder`,
`RxBaStarted`, `RxBaStopped` and `TxRateSelected`; all are in `Observation`. Receive events originate in the upper-MAC BA owner, using
the caller's temporarily borrowed observer (`view.observed(&observer)`). Its
placed storage keeps no observer state or diagnostic counters. The runtime
passes its existing observer to receive, negotiation, deadlines and clear.

Containment retains hardware/protocol/unavailable provenance independently
for A-MPDU and S-MPDU and the admitted MPDU byte length (explicitly unavailable
if unrepresentable). It counts admitted MPDUs, not physical aggregates. The S31
metadata provides no physical aggregate member count. Reorder decisions carry
held, released and missing counts; gap timeout releases buffered MPDUs and
skips missing sequences. Clear recycles its buffered owners. Released means
released by the reorder owner, not application delivery. Buffered duplicates
and other rejected/stale input are distinct reasons. BA events denote logical
owner activation/retirement, not confirmation of a hardware register write.

The station rate wrapper reports the selected first data-plan rate in kb/s
after association and each learned data terminal; an unavailable plan is
explicit. Computing that diagnostic plan is entirely inside the observation
point. Image collectors own weighted totals and last-rate state; bounded trace
history is not the source of those totals. None of these observations enables
TX A-MPDU: the ordinary S31 station port still submits individual attempts.

Images count these small values with atomic event-kind counters. Production observation has no record ring or frame snapshots; the bounded recording observer is a host test fixture only.
