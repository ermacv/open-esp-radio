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

- The gates were set by the predecessor's agents in its scenario files. The
  owner set only these:
  - the secure-GATT connection-loss bound;
  - the PHY calibration rule (the vendor's own spread);
  - for an A/B comparison against a predecessor build: 5 of 5 passes, a
    throughput regression above 5 % blocks, a p95 latency increase above 10 %
    blocks, and CPU load is compared.
- The runner, receipts and qualification procedure are not carried; only what
  each scenario proves and its gate.
- Diagnostic-image scenarios (probes, samplers, traces) are left out; see the
  end of [wifi.md](wifi.md).
