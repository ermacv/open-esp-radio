# ESP32-S31 acceptance criteria

These files list hardware-in-the-loop scenarios of an ESP32-S31 stack
outside this repository, as its catalog stood on 2026-10-16. They are the
acceptance criteria of the capabilities: a HIL test of a capability proves at
least what the matching scenario proved, against at least its gate.

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

- **"Pass"** gives the latest stand run that passed, and a measured value
  where one was recorded. The run records are outside this repository, and
  no result here proves code in it.
- **"Flaky"** means the scenario also failed often.
- **"Not confirmed"** means no hardware result was found.

## Notes

- The gates are those of the catalog's scenario files, except these:
  - A/B comparison against a reference build: 5 of 5 runs; a throughput
    regression above 5 %, a p95 latency regression above 10 % or more
    per-core CPU load for the same throughput blocks;
  - secure GATT: connection loss at most 20 % over at least 45 attempts;
  - GATT trace: 10 repetitions;
  - PHY calibration tolerance: a field matches when its difference stays
    within the vendor's own spread for that field;
  - GATT and secure GATT 10 of 10: open.
- The files hold what each scenario proves and its gate, not the catalog's
  runner, receipts or qualification procedure.
- Diagnostic-image scenarios (probes, samplers, traces) are left out; see the
  end of [wifi.md](wifi.md).

## Known gaps of the catalog's scenarios

New scenarios do not repeat these:

- Station TCP floors were fixed (RX 18, TX 45 Mbit/s) and ignored the
  offered load.
- "Roundtrip" ran the same code as "monitor", so it proved nothing more.
- The TX path required at least 8 s of traffic while scenarios allowed a 5 s
  window.
- PMF and WPA3 were verified only across a role restart, not on a first
  connection.
- `maximum_rx_silence_ms` was declared but never enforced.
