# Wi-Fi access-point scenario

`cargo hil run wifi-access-point` checks the Wi-Fi access point of the
application entry `oer` (feature `access-point`: its access point and IP
service, a static IPv4 address) on the `hil-wifi-access-point` image
through the [Wi-Fi access-point service](../../services/wifi-access-point).
Every claim of the device is checked against the stand's Linux station, the
`linux-net` fixture's `wlan0` ([fixture](../../../tools/lab/linux-net/README.md)).
The run leases the board and the air exclusively
(`Scenario::using_air_exclusively`): no other lease uses the station
meanwhile.

| Case | Checks |
| --- | --- |
| `serve` | The device serves a generated WPA2-Personal BSS (CCMP) on the first channel the stand file's `[router]` allows and answers once it beacons; its status names the BSSID and its static address `192.168.4.1/24` |
| `beacons` | Over a three-second window, the beacons delivered and expired equal the window's TBTTs at 100 TU within two, one for each status request's latency; none expired |
| `associate` | The fixture's station (WPA2, HT, static `192.168.4.2/24`) associates and completes the 4-way handshake: its supplicant reaches `COMPLETED` on the device's BSSID, and the device counts one station associated and authorized |
| `ping` | The station pings `192.168.4.1` twenty times at 200 ms; at least nineteen replies |
| `stop` | The device stops: it no longer beacons and counts no station |

The cases run in order on one boot: each needs what the one before did.
The passphrase reaches the fixture only on its standard input; the
fixture keeps its supplicant configuration in its private runtime
directory. After the run the scenario stops the fixture's station and
returns `wlan0` to its managed state (`open-radio-net stop`, `managed`).

```console
cargo test -p oer-hil-scenarios-wifi-access-point
```
