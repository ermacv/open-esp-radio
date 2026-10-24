# Wi-Fi HIL lifecycle and traffic

## Station wire module

The `wifi` feature defines `wifi/station/start`, answered by `base::Accepted`
for admission, and `wifi/station/link` for `Associated`, `Authorized` and typed
`Disconnected` states. A link event carries a station-attempt epoch within its
boot. `Authorized` means installed RSN keys and an open controlled port.
`StationPowerSave` admits only `Off`.

`ProbeStation` admits a 100-echo or ten-echo ICMP window. Its correlated
`StationProbeReady` precedes traffic; `StationProbeFinished` contains distinct
target-side request/reply counts. `StationGroupKey` reports a successful
hardware installation's public KeyID/generation, never the GTK.
`GetStationCounters` returns timeouts, collisions, decoded group ARPs and
departure counts. `StopStation` is accepted before disconnect/stop; only a
clean service stop publishes `StationStopped`. None carries a gate verdict.

`NetworkCredentials` owns an SSID of 1..=32 bytes and a passphrase of 8..=63
bytes. Construction and binary decoding both enforce these bounds. It has no
`Clone` or `Copy`; drop erases its storage. `clear_passphrase` erases the entire
passphrase buffer, preserves the SSID, and prevents reusing the cleared owner
in another wire command. Debug and human-readable serialization replace the
credentials with `<redacted>`; human-readable decoding is rejected. The host
records command identity and time, never command payloads. Transport Debug
prints metadata and payload length only. A target must not echo credential
wire bytes into UART evidence.

## Lifecycle and traffic contract

Boot is role-neutral:

```text
Hello -> WaitingForInitialization
      <- optional calibration chunks
      <- Initialize(IPv4 policy)
      -> Initialized + WifiIdle
      <- StartStation(credentials, power save) | StartAccessPoint(credentials, HT channel, static IPv4)
       | ScanWifi | StartMonitor | CaptureMonitor
```

Credentials exist only in role commands, are bounded/redacted/zeroized and
never enter scenario files or logs. AP IPv4 configuration is applied by the
HIL application to its persistent network stack, not by the radio driver.
Calibration bytes are opaque, chunked and CRC-protected; the host persists
them, never target NVS/flash.

RX admission follows the compiled sink's ownership contract. A receiver with
a separate output pool waits for its queue and buffer credits while retaining
the original staging owner. Initialization cannot override that requirement
or select direct dispatch for a sink which lacks immediate publication credit.

Traffic uses one state machine for UDP/TCP and RX/TX/bidirectional:

```text
Configure -> Arm -> Start -> SessionReady -> Evidence -> Finished(CRC)
          -> ReplayResult -> Evidence -> Finished(same CRC)
          -> AcknowledgeResult -> Idle
```

UDP `network::ServiceReady` is published after the socket is bound. The host waits for
both this declaration and `network::Ready` for the same interface before sending
an unmeasured `UdpProbe` challenge on the exact TX flow. Only its matching
nonce response confirms the reverse path; a successful host `send()` does not.
The target services probes while idle, before `network::Start`, using its bound TX
socket. Requests retry on a deadline within an absolute failure bound. Probe
responses never contribute to measured sequence counts, including late copies.
The host starts its socket collectors before `network::Start`. `network::SessionReady` confirms that the requested
workers and link preconditions are ready for the measured session. Readiness
has no fallback IP, settle delay or success-on-timeout path. USB serialization
completion and BlockAck readiness wake their waiters on state changes.
The host serial reactor wakes on descriptor readiness, queued commands or
shutdown. Protocol waiters also receive cancellation notifications; their
timeout is a failure bound rather than a periodic readiness poll.

`network::Finished` counts stack admissions, not on-air delivery. Host UDP collectors
use its TX count to complete as soon as every datagram arrives, or record a
delivery deadline and the remaining deficit.

In the other direction the host's count is the agreement. The host's paced
sender sends one datagram in each packet-interval slot that starts inside the
session's duration, then redundant terminal markers. The target measures
throughput over exactly the duration from the first datagram (`rx_bytes`,
`rx_units`), and counts every datagram that arrives after that window and
before the terminal marker ends the session as `rx_late_bytes` and
`rx_late_units`. Late datagrams are delivered, and their sequence numbers
count toward ordering. An exact delivery compares the host's count with
`rx_delivered_units()`, the sum of both, and never with the window's alone:
a datagram that waited in a queue is late, not lost. A session whose marker
never arrives ends at the grace deadline, and what is missing then is a
loss. A missing terminal result and a
fully delivered stream are distinct outcomes. Each `*-reception.json` records
Linux socket drop deltas (`SO_MEMINFO`), including losses with no later packet.
A nonzero delta invalidates the measurement as `host-overflow`; unavailable
accounting on another platform is `null`, never an asserted zero.

The target retains the complete result before its first publication, including
fixed link and stack snapshots. `network::ReplayResult` changes only the response
envelope identity and sequence, not evidence or its digest. The host verifies
the replay before acknowledging removal of the retained result.

Evidence is typed. Every session includes transport, UART link health and CPU
stack watermarks. UDP adds the radio facts needed for qualification; the RX
diagnostic image also adds delivery-frontier evidence. Aggregate histograms and
TX timing have typed records; supplemental text diagnostics cannot establish
readiness or completion. An image with `telemetry/cpu-load` adds
`CpuLoadEvidence` to every TCP session and every one-direction UDP session:
the window's cycles and each core's cycles in its executor's `wfi` over the
throughput's measurement window: for UDP RX from the first datagram, or the
end of the 2-second startup allowance when none arrived by then, to the
window's end; for UDP TX and TCP the elapsed interval. A core asleep at a window boundary is woken once so that its wait
counts up to the boundary; a core that does not answer is `None`. A
bidirectional UDP session measures one window per direction and adds none; a
TCP session, served by one task in either direction, measures one.

TX evidence covers a live interval, not a guessed queue drain. The radio executor
collects the two aggregate lifecycle snapshots between its polls. Publication
and prepared-standby owners still outstanding at each boundary are explicit.
For a failure-free interval, `publications + pending_start = block_ack_samples +
pending_end`; likewise, `standby_prepared + standby_pending_start =
standby_published + standby_cancelled + standby_pending_end`. Terminal hardware
failures remain failures, and a missing completion cannot be replaced by a
numeric tolerance. TCP and UDP derive text and typed aggregate evidence from
the same frozen snapshot.

An uncertain host response is resolved without guessing:

```text
GetStatus -> OperationStatus
ReplayResult | Cancel (before Start) | Recover (terminal state)
```

Target event sequence continuity and both endpoint decoder counters are part
of the result contract. The host stores exact received bytes in `uart.bin`, a
lossy text view in `uart.log`, and decoded events plus link/finalization health
in `protocol.jsonl`. Commands are omitted because they can carry secrets.
A capture owns one boot; transport loss or an unexpected reboot invalidates
outstanding operations and wakes their waiters. Optional waits return no event
only while the link remains healthy.

Wi-Fi commands admit only operations valid for the current `WifiIdle`,
`WifiStation`, `WifiAccessPoint` or `WifiMonitor` owner. Admission, successful
completion and terminal role failure are distinct request-correlated events.

The restart report carries the next radio-role generation after station stop
and what the shared radio did: `ClosedAndWoken` when Wi-Fi was the last PHY
client, or `KeptOpen` when another client kept RF open. A Wi-Fi-only image
requires `ClosedAndWoken`. These logical values do not by themselves prove
physical RF restoration or quiescence. Station lifecycle
generation is a separate link-epoch counter: a stopped connected STA must
publish a fresh `LinkPolicy` disconnect for the previous link epoch, then a
new `Connected` edge in the next link epoch. A connected edge includes the
actually negotiated association width and security from the production station
status snapshot. Missing metadata cannot establish HT40/WPA2-Personal.

A `Faulted` edge ends the station lifecycle of the boot: the station role
faulted with a non-reusable owner, in the phase and for the cause of the
portable station fault reason. No readiness follows it, and the host fails a
station readiness wait with that reason instead of a timeout.

The lifecycle scenarios require fresh station `network::Ready` after each
reconnection, then a bounded bidirectional UDP application session before and
after each radio cycle. Its optional `SessionFlowConfig.payload_identity`
binds the payload to the current boot and session ID. The target counts only
consumed RX datagrams with matching identity and fill; the Host checks each
received target TX datagram's source, length, identity, fill and sequence, and
reconciles both directions with retained session evidence. `network::ServiceReady`,
`network::SessionReady`, a probe response or a previous network address cannot replace
that payload proof. Other sessions leave `payload_identity` unset and retain
their existing traffic format. Hardware execution and dated link evidence are
still required for radio-cycle qualification.

## Terminal TX evidence

Station `TxRadioEvidence.station_terminal` reports logical aggregate receipts
only after BlockAck retries and any detached ordinary retry have terminated.
`acknowledged + unacknowledged == mpdus` uses wrapping interval counters;
invalid normalized statuses are counted separately and invalidate evidence.
These counters exclude live and quarantined exchanges. A missing ACK does not
prove that the peer failed to receive an MPDU. They are neither UDP sequence
accounting nor a radio-drain barrier. The host retains them independently in
`station-tx-terminal.json`; aggregate publication/completion counters retain
their earlier per-publication meanings.
