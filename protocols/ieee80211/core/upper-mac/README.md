# IEEE 802.11 upper-MAC state and policy

`oer-ieee80211-core-upper-mac` holds the transmit decisions that lie above the
[lower-MAC port](../../contracts/port/README.md): one port submission is one hardware
transmission attempt, and everything a backend does not report as a
`HardwareServices` offload is decided here, once for every backend, except
the contention draw, which each backend does for its queues below the port. The
package is sans-IO: completions and the radio time enter as values, attempt
plans and reports leave as values, and nothing waits.

| Module | Decides | Inputs |
| --- | --- | --- |
| `retry` | Whether an MPDU is sent again, whether that attempt sets the Retry bit, the counter a frame's class advances (`FrameClass`), the ladder step of its failures (`ladder_step`) and the rate of each attempt; a backend drawing its own contention derives the window's reset or doubling from that `RetryDecision` | `TxStatus`, `RetryCounters`, `RetryLimits`, a `RateLadder` |
| `airtime` | Which eligible radio peer receives the next bounded exchange grant, accounting for reservations, retries, cancellation and association retirement | Generic peer keys, minimum exchange costs, modelled terminal charges |
| `aggregate` | How many queued frames one A-MPDU carries: the agreement's window, the port's capabilities, the recipient's Maximum A-MPDU Length and Minimum MPDU Start Spacing, and the TXOP limit with its BlockAck | `AmpduLimits` |
| `ampdu` | Which subframes of an A-MPDU the next attempt resends after its BlockAck, republication after a failed protection exchange, the BlockAckReq after repeated failures, individual retries, aging and the end of the exchange | `BlockAckReport`, `AmpduRetryPolicy` |
| `rate_control` | The first rate of a peer's MPDUs and A-MPDUs, one controller per peer (a station's access point, each station of an access point), learning from each exchange, starting from the `LinkMetric` (signal over the noise floor, required) of the frame that made the link | `RatePeer`, `TxReport` outcomes |
| `rate_selection` | The retry plan of each frame an interface sends, by what it is for (`FrameRates`: probe, control, data), learning from each terminal and from the association with its peer; `RateSelection` is the seam a service drives, over the port's receive metadata and ACK evidence | `RatePeer`, `SingleOutcome` |
| `protection` | RTS/CTS or CTS-to-self and the control-frame rate for each PPDU, as the port's `Protection`, with the BSS facts it reads (`BssProtection`, `BasicRates`) | `BssProtection`, dot11RTSThreshold, an `HeTxopRtsBudget` |
| `block_ack` | What a `BlockAckReport` acknowledges, under both starting-sequence conventions; agreement negotiation, an association's receive agreements and their reordering | `BlockAckReport`, received ADDBA, DELBA and BlockAckReq frames |
| `management` | The attempt budget of a management frame: seven for unicast (dot11ShortRetryLimit), one for broadcast | a legacy rate |
| `phy_mode` | The PHY mode of one association and the capability elements its request carries (`AssociationPhy`) | a `ScanRecord`, `AssociationCapabilities` |
| `tx` | `TxPlanner`: the next `TxAttemptPlan` (rate, protection, power, coexistence, content and Retry bits) of an MPDU or A-MPDU exchange, and its `TxReport` | `TxRequest`, `PublicationReport`, `RadioInstant` |

The planner draws no backoff: the lower-MAC port carries none, and a backend
draws its queues' contention below the port, resetting the window when
`MpduRetryState` ends a frame and doubling it when it retries one. Reports
are the port's `MacTxStatus` for an MPDU and this package's
`MacAmpduTxStatus` for an A-MPDU. The checked `WmmAccessCategory` of a
request passes through to each `TxAttemptPlan::access_category`.

`TxExchange` retains its immutable request context once. The current phase
holds the sole MPDU/A-MPDU description, including the aggregate lengths;
the finished phase retains that description with its report. `request()`
reconstructs a value snapshot rather than borrowing a second stored copy.

`RateControl` observes a `NonZeroU16` transmission count with `MpduOutcome`
for an ordinary MPDU, or a checked `BlockAckTally` for an aggregate. A tally
cannot contain zero attempts or more acknowledgements than attempts. The
controller returns `AmpduUnavailable` directly when it has no aggregate
state, before changing its rate or ACK-SNR filter.

`RatePeer` retains the nonempty `NegotiatedLegacyRates` from both rate elements,
including non-basic rates. This set bounds legacy data, control and retry
fallback on every PHY. An association with no recognized legacy rate is
refused; `PhyMode::Legacy` alone does not imply OFDM support.

`rate_selection::management_rate(band, Option<&BasicRates>)` derives the
management/group rate above the driver: the lowest advertised basic rate
the band admits. Without a known BSS it selects DSSS 1 Mb/s in 2.4 GHz or
OFDM 6 Mb/s in 5 GHz. A supplied empty or incompatible set refuses TX;
6 Mb/s is selected before 11 Mb/s CCK when both are basic. The caller wraps
the result in `management::unicast` (at most seven attempts) or
`management::broadcast` (one), a port `ManagementTxPlan` that keeps that rate
for every attempt. `BasicRates` reads the wire's Supported Rates codec
(`oer_ieee80211_wire::rates`), as `NegotiatedLegacyRates` does.

`airtime::AirtimeStorage` owns a bounded, allocation-free deficit ledger.
Its scheduler reserves credit before publication; unpublished work may be
cancelled, while published work retains an affine completion owner until the
caller supplies its modelled charge. Keys include the association generation,
not a transport flow. Idle peers lose positive surplus but retain debt;
retired accounts remain pinned until their outstanding work is settled.
`reserve`, `settle` and `cancel` report each successful transition as an
`AirtimeObservation` to the caller's `EventSink`; the ledger holds no sink.
The module accepts generic receipts and has no chip, ESP-NOW or packet-storage
dependency. It has no production consumer. Its unit tests and compile-fail
doc-tests live here; integration tests with chip work receipts, cost estimates
and network TX queues live in the S31 MAC package.

## Parameters and profiles

What differs between implementations is a parameter, never a branch on a
vendor:

- `RetryLimits`: dot11ShortRetryLimit, dot11LongRetryLimit and whether an
  ACK timeout counts against the short or the frame-class counter
  (`RetryLimits::IEEE_DEFAULT` is seven, four and by frame class).
- `RateLadder`: the rate of the attempt after `n` failures, supplied by the
  rate-control owner.
- `AmpduRetryPolicy`: MSDU lifetime and aging margin, the retry limit of an
  unanswered aggregate and of failed protection exchanges, and whether one
  missing subframe stays aggregated.
- `HeTxopRtsBudget`: how many HE SU APEP octets fit below a TXOP duration
  threshold; `ProtectEveryHeTxop` protects every individual HE PPDU while a
  threshold is advertised.

[`oer-espressif-ieee80211-policy`](../../../../chips/espressif/ieee80211-policy/README.md)
supplies the Espressif values recovered from the vendor stack.

Control-rate selection starts from the first eligible advertised basic rate.
When none is eligible it starts from the ERP mandatory set's compile-time
checked head; its selection fold always has a candidate. It then chooses the
fastest candidate no faster than the data rate, or the slowest candidate when
all are faster, applying the BSS preamble rule afterward.

Every subframe ages from its MSDU's admission into software, on the
monotonic clock (`oer_time::Instant`, `protocols/ieee80211/contracts/port/README.md`, MSDU age): the
caller gives each original subframe's admission instant with every
completion, and a subframe without one ends the exchange as incomplete
(`UnknownSubframe`), never a panic. A subframe is aged once less than the
aging margin of its lifetime is left. Every retry outcome observes it,
including a protection failure before any subframe reached the peer, and
the individual retries of a subframe that left the aggregate. An aged
subframe is discarded instead of retried; under an operational Block Ack
agreement a BlockAckReq then moves the recipient's window past it
(`DiscardAged`), starting at the first remaining subframe or after the last
discarded one. Without the agreement no BlockAckReq goes.

`AmpduRequest` owns its bounded lengths; asking for a subframe outside that
request gives `None`, and the planner reports `SubframeOutsideAggregate`
before publishing an attempt. An unrepresentable subframe size admits no
frame and leaves the aggregate run unchanged. `AmpduLimits` includes aligned
empty delimiters between MPDUs for the peer's start spacing at the exact
HT/HE symbol rate, in both the byte and TXOP bounds. The final MPDU needs no
spacing tail. Admission and backend encoders share checked delimiter geometry
from `oer-ieee80211-wire::ampdu`; each encoder separately applies its hardware
limits. `ampdu::compact` operates on
fixed arrays of Copy metadata, never on affine MPDUs or payload owners.
It swaps selected entries into the prefix in order, without a full-array
temporary. Only the first 64 entries are addressable by its mask; callers
consume the returned prefix length and do not inspect the remaining suffix.
The crate uses the canonical strict lint block.

## Protocol state

`block_ack` owns session negotiation, the originator window and the receive
agreements: `recipient` negotiates one agreement (ADDBA parameters, typed
refusals, inactivity and the reorder gap) and `banks` owns an association's
agreements, their retained frames and the replies they owe. `banks` parses
ADDBA, DELBA and the Compressed BlockAckReq and selects the QoS Data it
reorders with the wire codecs; it calls the port only through the caller's
`RxBlockAckPort` (install an agreement, end one, return a frame), which the
runtime implements over its phase. A credit stays free to close a reorder
gap, and a management Action whose reply would find no place waits retained. `ccmp` owns transmit PN allocation and receive replay frontiers for every
CCMP user: one strictly increasing `CcmpReplayFrontier` per lane, key or
peer, borrowed by an admission candidate until commit, for the station's
data, the access point's and the RSN receiver's protected management frames
and ESP-NOW peers; key material, headers and PN values are wire vocabulary.
`sequence` owns the transmit sequence spaces (`SequenceCounter`,
`TxSequenceCounters`) of the station, the access point and ESP-NOW. `duplicate` owns
receive duplicate history, `fragmentation` bounded MSDU reassembly and its
reassembly refusals (`OpenDataReassemblyError`, over the wire's frame
refusals), and `receive` the replay commit boundary. `ccmp`'s
`CcmpPacketNumberStep` is how far a key's transmit PN advances per MPDU.
`qos` classifies outgoing Ethernet frames into user priorities and access
categories and downgrades an access category under admission control.
`rate_selection::RateSelection::learn` routes a terminal's sample to the
record of its frame class (probe, control, data), only for a current link's
completion that carries one. `monitor` selects the frames a monitor keeps and
the channels it visits, and revalidates a retained injection request; `vif`
routes a received MPDU of a same-channel station and SoftAP pair to its
interface from its public header fields. These owners accept decoded wire
values and retain no executor, HAL or chip dependency.

Each TX Block Ack session tracks delivery and expiry in two 64-bit masks,
relative to its current originator-window head. Negotiated windows larger
than 64 are refused. `on_delivered` and `on_expired` apply every mark in a
report before advancing past the contiguous completed prefix. Non-head
expiry stays recorded until the head reaches it, including after later
deliveries. Passing any expired sequence produces one BAR at the final
window start. Both originator methods return that frame for the caller to
submit once; the backend only reports outcomes. Sequence arithmetic wraps
through zero, and stale reports cannot move the window again.

`packet_number` supplies checked 48-bit advancement and a borrowed commit
frontier to both CCMP and RSN's BIP owners. Its value parameter keeps
`CcmpPacketNumber` and `BipPacketNumber` distinct; a candidate cannot outlive
or be transplanted to another frontier. Cancelling a candidate advances
nothing. BIP transmit candidates commit only after frame protection succeeds;
receive candidates commit only after authentication.

## What stays outside

The planner holds no frame bytes. The caller encodes each MPDU's sequence
number and reserves its CCMP PN with the key's unique `ccmp::CcmpTxPacketNumber`.
`ccmp::request_ccmp_pairwise_key` enqueues new material through the port and
returns a `PendingCcmpTxKey` holding the installation identity the port
returned; only that token, spent with the key the installation terminal
returns, creates the allocator and retains it in `InstalledCcmpTxKey` with
that backend key. A key whose `pairwise_identity` differs, another
installation's, is refused as `ForeignCcmpTxKey` with the token and the key;
`InstalledCcmpTxKey::install` does the same for a `SoftwareCcmpKey`, key
material moved into its software owner (an ESP-NOW peer), and `replace_key`
replaces that material without restarting its numbers. `number` hands out a
fresh MPDU's port `NumberedKey`.
The allocator has no public constructor or continuation/reset API.
The backend formats that supplied PN at the first protected transmission;
a retransmission sends the same MPDU and keeps both. `allocate_batch`
reserves all fresh subframe numbers atomically before an aggregate handoff. The
receive reorder buffer is `block_ack::reorder::RxReorderBuffer`. Adaptive
rate control, which chooses a frame's first rate from the reports, is the
caller's.

`retry` owns portable retry state and a detached rate-series selector over
`RateLadder`; neither retains backend policy. `backend_aggregate` owns bounded
aggregate retry state; `block_ack::batch` returns each retained MPDU exactly
once. Its `next_completion` returns a typed `MissingCompletion` with the
unchanged completing owner if a retained entry is absent; the caller keeps
that owner and its remaining frames. Reorder admission resolves the incoming
frame's array position before any state changes and returns `InvalidPosition`
with its position and capacity if that internal proof fails.
The family rate controller selects rates above the backend. Protection
selection stays here and passes its exact control-frame rate to the port.

## Known defects

- **Untyped TID:** `ccmp.rs::CcmpReplayError::InvalidTid` exists because the
  TID is a raw value.
- **Low:** `fragmentation/reassembly.rs::preflight_in_epoch` checks
  `CONTEXTS == 0` at run time (`NoReassemblyContexts`) instead of a const
  assert; `ccmp.rs` has `impl Default for CcmpRxReplayState`.

The receive BA view's `receive`, `offer`, `tick` and `clear` take the
caller's `&mut impl EventSink<RxBlockAckEvent>` and report containment
evidence, reorder decisions with their held, released and missing counts, and
agreement start and stop; the view and its placed backing hold no sink. With
`()` no event is built and no metadata is read for one. The caller owns all
resulting diagnostic counters. No event changes an agreement, receive
disposition, gap deadline or frame owner.
