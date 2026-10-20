# Radio ports

A radio port is the contract between the protocol logic of one radio
protocol and the backend that executes it: a chip, a family driver or a host
model. Everything above a port is written once for every backend; everything
below it is the backend's. The ports are not frozen; this page states the
principles the port crates follow. The shared lifecycle and vocabulary are in
[`oer-radio-port`](../protocols/radio/port/README.md), the Wi-Fi port in
[`oer-ieee80211-port`](../protocols/ieee80211/port/README.md).

## Placement follows hardware autonomy

Work the backend performs without software on the air timeline
(acknowledgement turnaround, FCS or CRC, hardware retransmission, hardware
ciphers) lies below the port. Work that software decides (retry policy, rate
selection, reordering, sequence numbers, scanning, beacons, power-save policy)
lies above it, in portable code. A backend that also performs work above the
port reports it in its capabilities, and the portable owner delegates instead
of doing it. For IEEE 802.11 one publication is one hardware transmission
attempt and the caller chooses the retry plan; the contention draw and the
numbering of protected MPDUs are the two exceptions the backend always does
below the port. For IEEE 802.15.4 the acknowledgement (frame pending and the
enhanced ACK's contents), CSMA-CA and the cipher run below the port from
tables and parameters the core sets through it; frame retries stay above.
For Bluetooth LE the backend reports its event timing allowances, and the
link layer above the port decides how long each event may last.

## Five parts of every port

| Part | Semantics |
| --- | --- |
| Submission | Immediate admission of one request with a caller-chosen correlation identity. A refusal is the call's result, never a later event, and a value, not a fault. Synchronous when the backend can decide without waiting; asynchronous only if the backend needs a fresh hardware reading, with a bounded wait that depends on no other submission or event |
| Events | One asynchronous stream of owned events with exactly one consumer; loss is reported (`EventsLost`), never silent |
| Capabilities | What the backend supports and does autonomously, read before submission |
| Lifecycle | Enable, disable and cancellation of submitted work, each with a terminal event; disable ends the work in flight first |
| Clock | The backend's radio time with its resolution and its relation to monotonic time (`ClockInfo`), see [time](time.md) |

- **Correlation identities.** Each port has its own 32-bit identity type, so a
  completion of one port cannot be taken for another's work. The top 256 raw
  values (`BACKEND_RESERVED`) are for work the backend submits itself; a
  caller's allocator wraps before them.
- **One consumer.** Users that share a port share it through a router that
  owns the stream and dispatches by identity. Taking an event only dequeues
  it. Timed work the backend does in software runs in its driver, which owns
  the radio by value and serves the port's requests as messages.
- **A port exists only while its backend is installed.** Installation hands
  out the port (for the protocol's consumer) and a control (for the
  composition: the backend's own operations such as statistics and diagnostic
  reads); uninstall consumes both, so "not installed" is never a refusal.
- **Capabilities in three places.** A structural optional feature (aggregate
  transmission, TBTT reporting) is an extension trait: a backend without it
  does not implement it, so it cannot be requested. Parametric limits (bands,
  rates, queues, key slots, window sizes) are capabilities; a value outside
  them is refused as unsupported and that refusal means nothing else. Why a
  backend lacks something is written in its documentation, not in code.
- **Shared RF path.** Protocols name themselves with `RadioClient`
  (`oer-radio-coex`) and name each request by the port's own `Operation`
  enum; the chip family's coexistence owner maps both onto its arbitration
  below the port. There is no urgency level.
- **Names.** `*Port` is a portable contract trait, `*Service` a portable state
  machine that consumes ports, `*Hardware` a chip or family register seam
  below a port.

Shared-PHY work (calibration, tracking, retune) belongs to the radio-system
actor ([architecture](architecture.md#radio-system)); it is not a lifecycle
operation of a port.
