# Wi-Fi station load scenario

`cargo hil run wifi-load` measures UDP load between this host and the
product Wi-Fi station of the `esp32s31-wifi-station` image, through the lab
router: host → Ethernet → OpenWrt AP → Wi-Fi → station, one hop over the
air. The host sends and receives the load; the image counts its end through
the [load service](../services/load) and reports its CPU. Only parameters
and counters cross the HIL link. RX and TX are named from the image's side.

| Case | Does |
| --- | --- |
| `path` | Checks the host's route to the station network (an explicit route through the router, its source address), has the router bring up a generated WPA2 network that forwards UDP between its stations and that address, joins, takes a DHCP address, and proves both directions with datagrams of the run: the host's probes come back echoed, the image's datagrams reach the host. Records the topology, addresses, MAC, BSSID, channel and route |
| `rx` | The host sends to the image |
| `tx` | The image sends to the host |
| `rx-tx` | Both at once, half the total each way |

Each load case offers 10, 30, 50, 65 and 100 Mbit/s in total (1400-byte
datagrams, 5 s each) and records per level: offered, sent and received
throughput, lost (sent less received), loss below the highest sequence,
reordered and duplicate datagrams, the sender's refused sends, RTT of 20
probes per second echoed by the image (min, median, p95, max, replies), and
each hart's busy share (1 − idle/window) with each executor's work share.
Executors' work spans overlap (preemption): only a hart's idle time gives its
utilisation. The router forwards the load: its CPU can limit it.

The measurements decide nothing: a case fails only when the path or an
exchange does not work. Each measurement is printed and the run's results
go to `target/hil/results/wifi-load-<unix time>.json`.

The host needs, once, a route to the station network through the router's
LAN address: `ip route add 10.231.0.0/24 via <router LAN address>` (or the
same as a permanent route of the wired connection).

```console
cargo test -p oer-hil-scenarios-wifi-load
```
