# IEEE 802.11 upper-MAC state and policy

`oer-ieee80211-upper-mac` holds the transmit decisions that lie above the
[lower-MAC port](../port/README.md): one port submission is one hardware
transmission attempt, and everything a backend does not report as a
`HardwareServices` offload is decided here, once for every backend, except
the contention draw, which each backend does for its queues below the port. The
package is sans-IO: completions and the radio time enter as values, attempt
plans and reports leave as values, and nothing waits.

| Module | Decides | Inputs |
| --- | --- | --- |
| `retry` | Whether an MPDU is sent again, whether that attempt sets the Retry bit, and the rate of each attempt; a backend drawing its own contention derives the window's reset or doubling from that `RetryDecision` | `TxStatus`, `RetryLimits`, a `RateLadder` |
| `aggregate` | How many queued frames one A-MPDU carries: the agreement's window, the port's capabilities, the recipient's Maximum A-MPDU Length and Minimum MPDU Start Spacing, and the TXOP limit with its BlockAck | `AmpduLimits` |
| `ampdu` | Which subframes of an A-MPDU the next attempt resends after its BlockAck, republication after a failed protection exchange, the BlockAckReq after repeated failures, individual retries, aging and the end of the exchange | `BlockAckReport`, `AmpduRetryPolicy` |
| `rate_control` | The first rate of a peer's MPDUs and A-MPDUs, one controller per peer (a station's access point, each station of an access point), learning from each exchange, starting from the `LinkMetric` (signal over the noise floor, required) of the frame that made the link | `RatePeer`, `TxReport` outcomes |
| `rate_selection` | The retry plan of each frame an interface sends, by what it is for (`FrameRates`: probe, control, data), learning from each terminal and from the association with its peer; `RateSelection` is the seam a service drives, over the port's receive metadata and ACK evidence | `RatePeer`, `SingleOutcome` |
| `protection` | RTS/CTS or CTS-to-self and the control-frame rate for each PPDU | `BssProtection`, dot11RTSThreshold, an `HeTxopRtsBudget` |
| `tx` | `TxPlanner`: the next `TxAttemptPlan` (rate, protection, power, coexistence, content and Retry bits) of an MPDU or A-MPDU exchange, and its `TxReport` | `TxRequest`, `PublicationReport`, `RadioInstant` |

The planner draws no backoff: the lower-MAC port carries none, and a backend
draws its queues' contention below the port with `oer-ieee80211-port`'s
`EdcaContention`, resetting the window when `MpduRetryState` ends a frame and
doubling it when it retries one. Reports are the port's service contract's
`MacTxStatus` and `MacAmpduTxStatus`. The checked `WmmAccessCategory` of a
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
the result in lower-MAC's `ManagementTxPlan::unicast` (at most seven
attempts) or `broadcast` (one), keeping that rate for every attempt.

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

[`oer-espressif-ieee80211-policy`](../../../chips/espressif/ieee80211-policy/README.md)
supplies the Espressif values recovered from the vendor stack.

Control-rate selection starts from the first eligible advertised basic rate.
When none is eligible it starts from the ERP mandatory set's compile-time
checked head; its selection fold always has a candidate. It then chooses the
fastest candidate no faster than the data rate, or the slowest candidate when
all are faster, applying the BSS preamble rule afterward.

Every subframe ages from its MSDU's admission into software, on the
monotonic clock (`oer_time::Instant`, `docs/decisions.md`, MSDU age): the
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

`block_ack` owns session negotiation, the originator window and receive
reordering. `ccmp` owns transmit PN allocation and receive replay frontiers;
key material, headers and PN values are wire vocabulary. `duplicate` owns
receive duplicate history, `fragmentation` bounded MSDU reassembly, and
`receive` the replay commit boundary. These owners accept decoded wire
values and retain no executor, HAL or chip dependency.

## What stays outside

The planner holds no frame bytes. The caller encodes each MPDU's sequence
number and reserves its CCMP PN with the key's unique `ccmp::CcmpTxPacketNumber`.
`ccmp::install_ccmp_pairwise_key` creates it only while installing new material
through the port and retains it in `InstalledCcmpTxKey` with that backend key.
The allocator has no public constructor or continuation/reset API.
The backend formats that supplied PN at the first protected transmission;
a retransmission sends the same MPDU and keeps both. `allocate_batch`
reserves all fresh subframe numbers atomically before an aggregate handoff. The
receive reorder buffer is `block_ack::reorder::RxReorderBuffer`. Adaptive
rate control, which chooses a frame's first rate from the reports, is the
caller's.

`backend_retry` owns ordinary retry state over a static port profile and
portable rate/queue traits. `backend_aggregate` owns bounded aggregate retry
state; `block_ack::batch` returns each retained MPDU exactly once, and
`block_ack::banks` pools receive reorder state across interfaces. The
Espressif profile implements `port::backend::BackendTxProfile` here and in
`oer-espressif-ieee80211-policy`; the image/runtime composes it with the S31
hardware backend. The chip crates consume only port/wire values.

## Known defects

- `protection.rs` asserts a constant with the `[(); bool as usize]` array hack instead of
  `const { assert!(..) }`.
- **Untyped TID:** `ccmp.rs::CcmpReplayError::InvalidTid` exists because the
  TID is a raw value.
- **Low:** `fragmentation/reassembly.rs::preflight_in_epoch` checks
  `CONTEXTS == 0` at run time (`NoReassemblyContexts`) instead of a const
  assert; `ccmp.rs` has `impl Default for CcmpRxReplayState`.
