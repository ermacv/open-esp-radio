# Bluetooth LE acceptance scenarios

HIL catalog outside this repository, state of 2026-10-16; its run records are outside this repository too. DTM is Direct Test Mode; the
Linux adapter is driven over the exclusive HCI user channel unless BlueZ is
named. "Qualification" marks the scenarios the catalog's qualification requires.

## DTM both ways against a Linux adapter (qualification)

- Checks: LE 1M DTM packets from the ESP32-S31 to the Linux adapter and back,
  with counts from each receiver. The adapter keeps one address and version
  across all helper runs.
  - Silence controls: the S31 receiving while the adapter is idle counts 0.
    The adapter receiving while the S31 is idle counts 0, or ends by Reset
    (some adapters do).
  - Quiet cycles: RX then Test End, then RX/Reset/RX/Test End. Every count is
    0 and every restart after Reset completes. Cleanup Reset succeeds.
- Gate:
  - 2 boots, at least 10 packets in each direction.
  - 100 quiet cycles per boot, which is 200 zero Test End counts and 100
    restarts.
- Fixed conditions: RF channel 0 (2402 MHz), PRBS9, 37 bytes, DTM v2, 300 ms
  quiet RX windows.
- Peer: Linux Bluetooth adapter; no second ESP.
- Hardware: pass, stand run 2026-10-12. An earlier run (2026-09-28)
  counted 80 packets S31→adapter with 100 quiet restarts, while adapter→S31
  Test End timed out on the adapter side.

## DTM against an ESP32-C5 vendor-Controller peer (qualification)

- Checks:
  - The S31 receives LE 1M, 2M, Coded S=8 and Coded S=2 from the peer.
  - The S31 transmits LE 1M and 2M, counted by the peer.
  - Silence controls: each side receiving while the other is idle counts 0.
  - Every leg completes and both sides are reset afterwards.
- Gate: at least 100 packets on each of the 6 legs.
- Fixed conditions: RF channel 19 (2440 MHz), PRBS9, 37 bytes, HCI LE
  Receiver/Transmitter Test v2, 500 ms per leg.
- Peer: ESP32-C5 running an ESP-IDF Controller-only application (vendor BLE
  Controller over VHCI, DTM enabled) driven by a line protocol.
- Hardware: pass, stand run 2026-10-12. LE 2M TX sent half the
  vendor's packet rate; the 100-packet floor still held.

## Scannable advertising answered by scan response

- Checks: S31 `ADV_SCAN_IND` whose fresh per-run name is carried only in the
  scan response. BlueZ active discovery learns that exact name, which proves
  the `SCAN_REQ` was answered. On failure a connectable `ADV_IND` control runs,
  for diagnosis only.
- Gate:
  - Name discovered within 15 s.
  - Advertising interval 100 ms (160 × 0.625 ms) on all three advertising
    channels.
- Peer: Linux adapter under BlueZ as the active scanner.
- Hardware: pass, stand run 2026-10-12.

## Directed advertising, high and low duty

- Checks:
  - High-duty `ADV_DIRECT_IND` to an absent random target ends with LE
    Connection Complete status 0x3C (advertising timeout).
  - Low-duty directed advertising to the adapter lets it connect: status 0,
    with the adapter's address as peer.
  - S31 Disconnect (reason 0x13) returns status 0.
- Gate:
  - High duty ends no earlier than 1200 ms (nominal 1.28 s).
  - Low-duty interval 100 ms. Both events arrive within 5 s.
- Peer: Linux adapter under BlueZ as the initiating central.
- Hardware: pass, stand run 2026-10-12. High duty timed out after
  1280 ms (2026-09-28).

## Plaintext GATT peripheral, three connection cycles

- Checks: on one Host/Controller epoch, each connection cycle does:
  - MTU exchange (23–517);
  - discovery of service 0xFFF0 and characteristic 0xFFF1 by UUID;
  - read of the previous value, write of a new one (0x31, 0x52, 0x73), and
    read-back;
  - graceful disconnect (0x13), then advertising again.

  The device counters (connections, disconnections, writes, reads ≥ 2 per
  cycle) agree with the peer. GATT stack headroom stays present on CPU0 in
  every sample.
- Gate:
  - 3 repetitions × 3 connections.
  - At most 3 attempts per connection, retried only when the device proves it
    never saw the connection (2 s confirmation window).
  - Each state transition completes within 8 s.
  - Connection loss before the first ATT exchange is recorded, not gated.
- Peer: Linux BlueZ kernel ATT socket as the central. Connection parameters:
  7.5 ms interval, latency 0, supervision timeout 2 s.
- Hardware: pass, stand run 2026-10-12. On 2026-10-11, 20 of 20
  passed, with 11 of 71 connection attempts lost on the shared air.

## Plaintext GATT with connection-step trace

- Checks: the plaintext GATT scenario on a trace image. Each connection's
  drained trace holds, in order:
  - an accepted indication;
  - the opening;
  - at least one admitted connection event;
  - at least one anchored event end;
  - the closing.

  The trace buffer never fills. Its purpose is to catch a connection lost
  before its first ATT exchange.
- Gate: 10 repetitions × 3 connections (30 traced connections), each cycle
  complete. Other limits as for plaintext GATT.
- Peer: Linux BlueZ.
- Hardware: pass, stand run 2026-10-12, with 66–68 connection events
  per cycle.

## Secure GATT: pairing, bonding, cold restart, bond-load failure (qualification)

- Checks: five connections per repetition.
  1. Plaintext read, write and CCCD write are denied with ATT error 0x05 or
     0x08.
  2. A declined Numeric Comparison.
  3. An accepted Numeric Comparison: device and BlueZ show the same value
     (≤ 999 999), bound to the boot and the request.
  4. Protected read, notify and write, then a bonded reconnect without a new
     comparison.
  5. Controller cold restart on the same SoC boot (one cold release, old HCI
     closed, bond kept, epoch + 1), then a cold reconnect without re-pairing.

  Then a bond-load failure is injected:
  - the Reset reader is withheld and nothing reboots;
  - this leads to a terminal cold close with exactly one bond-load failure,
    one more cold release and no restart;
  - the boot is unchanged for 1 s.

  The IRQ stack policy holds at start and end. The temporary Linux bond is
  removed on cleanup.
- Gate:
  - 9 repetitions × 5 connections = 45 attempts.
  - Connections not established / attempts ≤ 20 % (2000 basis points), with a
    denominator of at least 45. Rationale: the shared air loses up to 17 % of
    CONNECT_IND to foreign scanners.
  - Transition deadline 10 s; pairing prompt 15 s; fault close 10 s.
  - Retry rule as for plaintext GATT.
- Peer: Linux BlueZ with a DisplayYesNo agent as central and bonding peer.
- Hardware: pass, stand run 2026-10-12. On 2026-10-11, 10 of 10
  passed with 0 of 50 attempts lost. Flaky over its history: 7 of 22 runs
  failed.

## Secure GATT with an injected shutdown HCI read failure (qualification)

- Checks: the secure GATT flow, restarted through the GATT restart path. The
  last step injects a shutdown HCI read error, after which:
  - the old epoch, owners, counters and bonds are kept;
  - zero bond-load failures;
  - no cold release, restart or SoC reset for 1 s.
- Gate: 3 repetitions; fault deadline 10 s.
- Peer: Linux BlueZ.
- Hardware: pass, stand run 2026-10-10. Flaky: 6 of 7 runs failed
  over its history.

## Secure GATT with boundary-only IRQ sampling

- Checks: the secure GATT flow with interrupt sampling only at boundaries. It
  is a timing comparison, not memory-stress evidence.
- Gate: 3 repetitions; timeouts as for secure GATT.
- Peer: Linux BlueZ.
- Hardware: pass, stand run 2026-09-28, 3 of 3.
