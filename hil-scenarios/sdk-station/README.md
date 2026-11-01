# SDK station scenario

`cargo hil run sdk-station` checks the Wi-Fi station and IP service of the
application entry `oer-espressif` on the `esp32s31-sdk-station` image
through the [SDK station service](../services/sdk-station/src/lib.rs).
Every claim of the device is checked against the lab router, which the run
leases with the board and the air (`Scenario::using_router`), or against
this host's own TCP peer.

| Case | Checks |
| --- | --- |
| `wrong-passphrase` | The router brings up its isolated station network with a generated SSID and passphrase (WPA2-Personal, CCMP) on the first channel the stand file's `[router]` allows, forwarding TCP and UDP to this host; a connect with another passphrase fails in the 4-way handshake: the station's `HandshakeFailed`, or the access point's deauthentication with reason 15 (4-way handshake timeout); any other code fails the case with it. The same boot keeps answering, and the station holds no link |
| `join` | A connect with the network's passphrase links on the router's channel; the router's station table shows the station authenticated, associated and authorized |
| `dhcp` | The device's address wait ends with a lease; the router's lease for its MAC holds that address, and the router (`10.231.0.1`) is its gateway |
| `tcp` | The device opens a TCP connection through the router to an echo peer on this host's address toward the station network, sends a line and reads it back; the peer received that line from the device's address |
| `reconnect` | The router deauthenticates the station: the device's link-loss wait ends, the station rejoins by itself, takes a lease again (checked as in `dhcp`) and exchanges a line again (as in `tcp`) |
| `no-dhcp` | The router stops serving DHCP and deauthenticates the station: the station rejoins, its address wait times out after 20 s with the link up and the boot unchanged; the router serves again and the station takes a lease |

The cases run in order on one boot: each needs what the one before did; the
first that needs the network applies it, so `--case join` runs alone. A
reset of the device fails the case it happens in. The host must reach the
station network through the lab router (`ip route add 10.231.0.0/24 via
<router LAN address>`, once), and its firewall must accept the TCP
connection from the station network to the echo peer's ephemeral port.
After the run the LAB service disables the router's AP section again; the
next applied station profile serves DHCP again.

```console
cargo test -p oer-hil-scenarios-sdk-station
```
