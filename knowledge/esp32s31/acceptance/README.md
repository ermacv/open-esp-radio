# ESP32-S31 acceptance criteria

These files list the hardware-in-the-loop scenarios the predecessor stack ran
on the ESP32-S31, as it stood on 2026-10-16. They are the acceptance criteria
for future slices: when a slice rebuilds a capability, its HIL test should
prove at least what the matching scenario proved, against at least its gate.

For each scenario they give:

- what it checks;
- the numeric gate (thresholds, durations, counts, rates);
- the peer it needs;
- the last known hardware result.

| File | Radio |
| --- | --- |
| [wifi.md](wifi.md) | IEEE 802.11 station, access point, STA+AP, monitor, sleep |
| [ble.md](ble.md) | Bluetooth LE: DTM, advertising, GATT, secure GATT |
| [ieee802154.md](ieee802154.md) | IEEE 802.15.4 and Thread |
| [coex.md](coex.md) | Wi-Fi with Bluetooth LE on the shared radio |
| [phy.md](phy.md) | Production calibration against the vendor's boot-to-boot spread |
| [system.md](system.md) | Boot, time, interrupts, IPC, panic, watchdogs, memory copy |

## Reading the status

- **"Pass"** gives the latest predecessor HIL run that passed, and a measured
  value where one was recorded. The last runs predate the predecessor's final
  tree, so no result here proves current code.
- **"Flaky"** means the scenario also failed often over its history.
- **"Not confirmed"** means no hardware result was found.

## Notes

- The gates were set by the predecessor's agents in its scenario files,
  except these, whose status is:
  - A/B comparison against a predecessor build: 5 of 5 passes, a throughput
    regression above 5 % blocks, a p95 latency increase above 10 % blocks,
    CPU load is compared — owner criterion (predecessor), confirmation
    pending;
  - secure GATT: connection loss at most 20 % over at least 45 attempts —
    owner criterion (predecessor), confirmation pending;
  - GATT trace: 10 repetitions — owner criterion (predecessor), confirmation
    pending;
  - PHY calibration tolerance (the vendor's own boot-to-boot spread) —
    agent-recorded, confirmation pending;
  - GATT and secure GATT 10 of 10 — open.
- The runner, receipts and qualification procedure are not carried; only what
  each scenario proves and its gate.
- Diagnostic-image scenarios (probes, samplers, traces) are left out; see the
  end of [wifi.md](wifi.md).

## Known gaps of the predecessor's scenarios

New scenarios should not repeat these:

- Station TCP floors were fixed (RX 18, TX 45 Mbit/s) and ignored the
  offered load.
- "Roundtrip" ran the same code as "monitor", so it proved nothing more.
- The TX path required at least 8 s of traffic while scenarios allowed a 5 s
  window.
- PMF and WPA3 were verified only across a role restart, not on a first
  connection.
- `maximum_rx_silence_ms` was declared but never enforced.
