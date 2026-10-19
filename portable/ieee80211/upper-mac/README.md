# IEEE 802.11 upper-MAC transmit policy

`oer-ieee80211-upper-mac` holds the transmit decisions that lie above the
[lower-MAC port](../lower-mac/README.md): one port submission is one hardware
transmission attempt, and everything a backend does not report as a
`HardwareServices` offload is decided here, once for every backend, except
the contention draw of its queues and the numbering of protected MPDUs,
which every backend does below the port without reporting them. The
package is sans-IO: completions and the radio time enter as values, attempt
plans and reports leave as values, and nothing waits.

| Module | Decides | Inputs |
| --- | --- | --- |
| `retry` | Whether an MPDU is sent again, whether that attempt sets the Retry bit, and the rate of each attempt; a backend drawing its own contention derives the window's reset or doubling from that `RetryDecision` | `TxStatus`, `RetryLimits`, a `RateLadder` |
| `aggregate` | How many queued frames one A-MPDU carries: the agreement's window, the port's capabilities, the recipient's Maximum A-MPDU Length and Minimum MPDU Start Spacing, and the TXOP limit with its BlockAck | `AmpduLimits` |
| `ampdu` | Which subframes of an A-MPDU the next attempt resends after its BlockAck, republication after a failed protection exchange, the BlockAckReq after repeated failures, individual retries, aging and the end of the exchange | `BlockAckReport`, `AmpduRetryPolicy` |
| `rate_control` | The first rate of a peer's MPDUs and A-MPDUs, one controller per peer (a station's access point, each station of an access point), learning from each exchange, starting from the `LinkMetric` (signal over the noise floor, required) of the frame that made the link; `FixedRateControl` | `RatePeer`, `TxReport` outcomes |
| `rate_selection` | The retry plan of each frame an interface sends, by what it is for (`FrameRates`: probe, control, data), learning from each terminal and from the association with its peer; `RateSelection` is the seam a service drives, over the port's receive metadata and ACK evidence | `RatePeer`, `SingleOutcome` |
| `protection` | RTS/CTS or CTS-to-self and the control-frame rate for each PPDU | `BssProtection`, dot11RTSThreshold, an `HeTxopRtsBudget` |
| `tx` | `TxPlanner`: the next `TxAttemptPlan` (rate, protection, power, coexistence, content and Retry bits) of an MPDU or A-MPDU exchange, and its `TxReport` | `TxRequest`, `PublicationReport`, `RadioInstant` |

The planner draws no backoff: the lower-MAC port carries none, and a backend
draws its queues' contention below the port with `oer-ieee80211-softmac`'s
`EdcaContention`, resetting the window when `MpduRetryState` ends a frame and
doubling it when it retries one. Reports are the softmac contract's
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

## Parameters and profiles

What differs between implementations is a parameter, never a branch on a
vendor:

- `RetryLimits`: dot11ShortRetryLimit, dot11LongRetryLimit and whether an
  ACK timeout counts against the short or the frame-class counter
  (`RetryLimits::IEEE_DEFAULT` is seven, four and by frame class).
- `RateLadder`: the rate of the attempt after `n` failures. `FixedRate`
  keeps the first rate; a rate-control owner supplies its own.
- `AmpduRetryPolicy`: MSDU lifetime and aging margin, the retry limit of an
  unanswered aggregate and of failed protection exchanges, and whether one
  missing subframe stays aggregated.
- `HeTxopRtsBudget`: how many HE SU APEP octets fit below a TXOP duration
  threshold; `ProtectEveryHeTxop` protects every individual HE PPDU while a
  threshold is advertised.

[`oer-espressif-ieee80211-policy`](../../espressif/ieee80211-policy/README.md)
supplies the Espressif values recovered from the vendor stack.

Control-rate selection starts from the first eligible advertised basic rate.
When none is eligible it starts from the ERP mandatory set's compile-time
checked head; its selection fold always has a candidate. It then chooses the
fastest candidate no faster than the data rate, or the slowest candidate when
all are faster, applying the BSS preamble rule afterward.

A-MPDU lifetime and aging margin are spans of the port's radio clock
(`Ieee80211Duration`). Aging uses the
same radio epoch as the commit instant and starts strictly after
`commit + lifetime - margin`; the exact first aged instant is checked before
admission. An unrepresentable threshold is `AgingThresholdOutsideEpoch`.
Every retry outcome observes that boundary, including a protection failure
before any subframe reached the peer: an aged aggregate issues neither another
publication nor a BlockAckReq. Its terminal retains the missing-subframe mask.
Backend adapters retain the commit stamp's generation and reject a sample
from another generation before consuming a completion or changing retry state.
Monotonic timers remain the source for waiting and transmit watchdogs.

`AmpduRequest` owns its bounded lengths; asking for a subframe outside that
request gives `None`, and the planner reports `SubframeOutsideAggregate`
before publishing an attempt. An unrepresentable subframe size admits no
frame and leaves the aggregate run unchanged. `AmpduLimits` includes aligned
empty delimiters between MPDUs for the peer's start spacing at the exact
HT/HE symbol rate, in both the byte and TXOP bounds. The final MPDU needs no
spacing tail. Admission and backend encoders share checked delimiter geometry
from `oer-ieee80211-mac::ampdu`; each encoder separately applies its hardware
limits. `ampdu::compact` operates on
fixed arrays of Copy metadata, never on affine MPDUs or payload owners.
It swaps selected entries into the prefix in order, without a full-array
temporary. Only the first 64 entries are addressable by its mask; callers
consume the returned prefix length and do not inspect the remaining suffix.
The crate uses the canonical strict lint block.

## What stays outside

The planner holds no frame bytes. The caller encodes each MPDU's sequence
number; the backend writes its CCMP packet number below the port at the first
protected transmission, with an `oer-ieee80211-mac::ccmp::CcmpTxPacketNumber`
of its key, and a retransmission sends the same MPDU, so it keeps both. The
receive reorder buffer (`oer-ieee80211-mac::block_ack::reorder`) is its
owner's in `oer-ieee80211-mac`. Adaptive rate control, which chooses a
frame's first rate from the reports, is the caller's.

The ESP32-S31 MAC (`oer-esp32s31-ieee80211-mac`) runs its ordinary retry,
A-MPDU retry and protection owners through these algorithms with adapters
from its chip rate and completion types.

## Known defects

- **Nit** (predecessor review, 2026-10-16): `protection.rs` asserts a
  constant with the `[(); bool as usize]` array hack instead of
  `const { assert!(../../../portable/ieee80211) }`.
