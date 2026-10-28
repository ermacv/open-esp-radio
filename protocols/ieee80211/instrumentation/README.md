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
| `SERVICE` | station service (`oer-ieee80211-runtime`) | `PortEventsLost`, `RxFrameDropped`, `RxMsduRefused`, `TxUnsent`, `TxMsduTooLong`, `GroupKeyInstalled` |
| `ARP` | station service | `ArpAddresses` followed by `ArpTransmitted` or `ArpReceived` |
| `ESP_NOW` | ESP-NOW encrypted peer table | `EspNowPeerTableChanged`, `EspNowPeersCleared`, `EspNowTxPacketNumber`, `EspNowReplay` |

An owner reports with `oer_trace::observe!` to its `O: Observer`, by default
`Noop`; without `cfg(oer_trace)` a point leaves no code. No owner keeps a
count of these events: a count is the number of events of a kind, which the
image's collector keeps. `ArpAddresses::parse` projects the RFC 826 fields of
an ARP MSDU inside the owner's observation point.

## Collectors and decoders

- `Observation::of(&event)` turns any event an observer receives into one
  typed value of this vocabulary (`None` for other vocabularies), so a
  collector's `Observer::event` is a `match`.
- `StationEvents` is the `oer_trace::EventSet` a host describes drained
  records with.
- `Recorder<N>` (feature `test-support`) keeps the first `N` observations in
  order, for host tests of owners and collectors under `--cfg oer_trace`.

## Checks

Unit tests (`src/tests.rs`): every event decodes to itself through its words
and the event set, kinds are unique and below the driver's range, channels
stay below the driver's, malformed words are refused, receive drop codes
follow the upper MAC's order, ARP parsing accepts only Ethernet/IPv4
requests and replies, and the recorder keeps observations only with
`cfg(oer_trace)`.
