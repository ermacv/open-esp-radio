# Wi-Fi station scenario

`cargo hil run wifi-station` checks the product Wi-Fi station
(`oer-esp32s31-wifi` with IPv4 over DHCP) on the `esp32s31-wifi-station`
image through the [Wi-Fi station service](../services/wifi-station). Every
claim of the device is checked against the lab router, which the run leases
with the board and the air (`Scenario::using_router`).

| Case | Checks |
| --- | --- |
| `join` | The router brings up its isolated station network with a generated SSID and passphrase (WPA2-Personal, CCMP) on the first channel the stand file's `[router]` allows; the device joins it, linked on that channel, and the router's station table shows it authenticated, associated and authorized |
| `dhcp` | The device takes an IPv4 address by DHCP; the router's lease for its MAC holds that address, and the router (`10.231.0.1`) is its gateway |
| `ping` | The router pings the device's address ten times; at least nine replies |
| `leave` | The device leaves: its link is down, and the router's station table no longer lists it |

The cases run in order on one boot: each needs what the one before did.
After the run the LAB service disables the router's AP section again.

```console
cargo test -p oer-hil-scenarios-wifi-station
```
