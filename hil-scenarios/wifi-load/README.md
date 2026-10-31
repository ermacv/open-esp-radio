# Wi-Fi station load scenario

`cargo hil run wifi-load` measures UDP load between this host and the
product Wi-Fi station of the `esp32s31-wifi-station` image, through the lab
router: host → Ethernet → OpenWrt AP → Wi-Fi → station, one hop over the
air. The host sends and receives the load; the image counts its end through
the [load service](../services/load) and reports its CPU. Only parameters
and counters cross the HIL link. RX and TX are named from the image's side.

| Case | Does |
| --- | --- |
| `path` | Checks the host's route to the station network (through the router's `[router].lan_address`, over a wired interface, from one source address), has the router bring up a generated WPA2 network that forwards UDP between its stations and that address, joins, takes a DHCP address, and proves both directions with datagrams of the run: the host's probes come back echoed, the image's datagrams reach the host. Records the topology, addresses, MAC, BSSID, channel and route, and the image's CPU over a 5 s window without load: the levels' baseline |
| `rx` | The host sends to the image |
| `tx` | The image sends to the host |
| `rx-tx` | Both at once, half the total each way |

Each load case offers 10, 30, 50 and 65 Mbit/s in total (1400-byte
datagrams); `OER_WIFI_LOAD_MBPS=10,65,100` replaces the levels, e.g. to
overload. Each level is one run of the image's load service: a 5 s
measuring window after 300 ms, then 500 ms of drain, after which the image
ends the run by itself. Throughput and CPU are over the window (the image's
clock); per level it records offered, sent and received throughput, lost
(sent less received), loss below the highest sequence, reordered, duplicate
and untracked datagrams (an exact tally, shared by both ends), the sender's
refused and missed sends, RTT of 20 probes per second within the window
(replies read through the drain) echoed by the image (min, median,
p95, max, replies), each hart's busy share (1 − idle/window) with each
executor's work share, each named part's polls (tasks, the station work's
futures and the station service's spans: count, rate, share of the window
with an open poll's part, and the mean and longest of the ended polls), and
the window's counters (the hart's cycles and retired instructions, their
rate per microsecond and per cycle, the network bridge's taken and refused
frames, and the IP stack's received frames with the hart's instructions over
them). The idle baseline records the parts and counters too. With
`OER_WIFI_LOAD_PROFILE_US=1000` (250 to 10000) each level also samples the
image's hart every period over the window and records a `profile`: ticks,
stride and effective period, the samples by interrupted raw CLIC level
(0 task code; with three level bits P1 `0x1f`, P2 `0x3f`, P3 `0x5f`, as
the CPU report's executor levels), the 40
functions and the 40 innermost inlined functions with the most samples, and
every raw sample as `[tick, pc, level]`. Ticks are due every period on the
timer's own schedule; one due while the hart's interrupts were masked
samples where they open. The functions are named from the DWARF of the
runtime ELF the harness built (`target/fw/esp32s31-wifi-station/runtime.elf`,
`oer_analysis_elf::dwarf::Symbolizer`); without it the profile records why
and keeps the raw samples. The scenario holds the air exclusively. Executors' work spans and
parts' polls include preemption and overlap: only a hart's idle time gives
its utilisation. The router forwards the load: its CPU can limit it.

The measurements decide nothing: a case fails only when the path or an
exchange does not work. Each measurement is printed, and after the lease's
release the run's results, its cleanup and verdict go to
`target/hil/results/wifi-load-<time in ns>.json`.

The host needs, once, a route to the station network through the router's
LAN address: `ip route add 10.231.0.0/24 via <router LAN address>` (or the
same as a permanent route of the wired connection), and the stand file names
that address as `[router].lan_address`.

```console
cargo test -p oer-hil-scenarios-wifi-load
```
