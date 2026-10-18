# Coexistence acceptance scenarios

Predecessor HIL catalog, state of 2026-10-16. These run a joint Wi-Fi and
Bluetooth LE image on the shared radio. Two IEEE 802.15.4 scenarios also
exercise coexistence; they are in [ieee802154.md](ieee802154.md).

## Station UDP receive beside a BLE ATT echo load

- Checks: the HT20 station receives UDP while one BLE connection runs an ATT
  write/read echo load for the same interval.
  - Every echo matches. The device counts equal the echo count.
  - The load sees no disconnection and ends with exactly one graceful
    disconnect (0x13).
- Gate:
  - 12 s window, 20 Mbit/s offered, 1472-byte payload.
  - Wi-Fi RX rate at least 8 Mbit/s.
  - At least 20 BLE echoes.
- Peer: Wi-Fi station fixture (the AP side offers paced UDP to the station)
  plus a Linux BlueZ adapter as the GATT central.
- Hardware: pass, predecessor HIL run 2026-10-11. On 2026-09-29 it measured
  12.88 Mbit/s Wi-Fi RX and 33 echoes.

## Shared-PHY tracking hold under Wi-Fi and BLE load

- Checks: the UDP and ATT echo load above, plus forced shared-PHY tracking
  ticks timed on the device.
  - Three configurations: RFPLL, calibration, both. Each is timed under load
    and again on an idle radio.
  - The periodic tracking timer suspends and then resumes.
  - Any failed, skipped or malformed tick fails the run.
- Gate:
  - As above: 12 s, 20 Mbit/s offered, at least 8 Mbit/s, at least 20 echoes.
  - 2 ticks per configuration. Hold durations are recorded, not bounded.
- Peer: Wi-Fi station fixture plus Linux BlueZ.
- Hardware: pass, predecessor HIL run 2026-10-11. Measured: bracket 15.8 ms,
  tick 30.1 ms.

## Wi-Fi radio restart while BLE advertises

- Checks: the HT40/WPA2 station is connected, and its data path is proven
  before each cycle. Each cycle then:
  - stops the station;
  - restarts the radio, which returns generation stopped + 1 and RF kept open,
    because the BLE GATT application holds RF;
  - restarts the station, which reaches link generation + 1 and passes the
    data path again.

  Radio and network work sit on different cores.
- Gate: 3 repetitions × 3 cycles, each within 90 s. No BLE numeric gate; BLE
  only advertises.
- Peer: Wi-Fi station fixture only.
- Hardware: pass, predecessor HIL run 2026-10-11.
