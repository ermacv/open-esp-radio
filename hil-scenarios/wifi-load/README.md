# Wi-Fi station load scenario

`cargo hil run wifi-load` measures UDP load between this host and the
product Wi-Fi station of the `esp32s31-wifi-station` image, through the lab
router: host → Ethernet → OpenWrt AP → Wi-Fi → station, one hop over the
air. The host sends and receives the load; the image counts its end through
the [load service](../services/load) and reports its CPU. Only parameters
and counters cross the HIL link. RX and TX are named from the image's side.

| Case | Does |
| --- | --- |
| `path` | Checks the host's route to the station network (through the router's `[router].lan_address`, over a wired interface, from one source address), has the router bring up a generated WPA2 network that forwards UDP between its stations and that address, joins, takes a DHCP address, and proves both directions with datagrams of the run: the host's probes come back echoed, the image's datagrams reach the host. Records the topology, addresses, MAC, BSSID, channel and route, and the image's CPU and link over a 5 s window without load: the levels' baseline |
| `rx` | The host sends to the image |
| `tx` | The image sends to the host |
| `rx-tx` | Both at once, half the total each way |
| `stop` | Leaves 2 s into an RX run of 65 Mbit/s and records how long the leave took. Fails unless the station answers `Down`: the image closes its load socket and stops its IP service, every lent receive buffer returns by the stop's 2 s deadline, and the radio retires. Runs last, because the image serves no station after a leave |

Each load case offers 10, 30, 50 and 65 Mbit/s in total (1400-byte
datagrams); `OER_WIFI_LOAD_MBPS=10,65,100` replaces the levels (and `OER_WIFI_LOAD_WIDTH=ht20` the 40 MHz channel the path case asks the router for), e.g. to
overload. Each level is one run of the image's load service: a 5 s
measuring window after 300 ms, then 500 ms of drain, after which the image
ends the run by itself. Throughput and CPU are over the window (the image's
clock); per level it records offered, sent and received throughput, lost
(sent less received), loss below the highest sequence, reordered, duplicate
and untracked datagrams (an exact tally, shared by both ends), the sender's
refused and missed sends, RTT of 20 probes per second within the window
(replies read through the drain) echoed by the image (min, median,
p95, max, replies; `OER_WIFI_LOAD_PROBES=off` sends no probes), each hart's busy share (1 − idle/window: the boot hart's
radio and station, hart 1's network and load) with each executor's work
share, each named part's polls (tasks, the station work's and the network's
futures and the station service's spans: count, rate, share of the window
with an open poll's part, and the mean and longest of the ended polls), and
the window's counters (the boot hart's cycles and retired instructions, their
rate per microsecond and per cycle, the network bridge's taken and refused
frames, the IP stack's received frames with the hart's instructions over
them, the L1 cache's `cache` counts and the receive path's `rx` counts).
The idle baseline records the parts, counters and link too.

`rx` holds the window's differences of the receive path, counted only by
a trace build (zeros otherwise). `driver` is the S31 receive driver's
`WifiRxCounters` (`available` false when they were not readable at both
ends): `mac_irqs`, staged `units` and `bytes`, `overload_drops` (bulk
units dropped, after the hardware acknowledged them, with only the
admission's critical reserve of staging left), `admission_blocks` (units
left at the ring head without a credit), `stage_capacity_blocks`, the
chained, oversized, empty and too-long drops, `service_passes`,
`service_errors`, `capacity_drains`, continuations taken at once or after
a wait, and the RX-control prefix's refusals: `rx_failures` (a failed RX
state, the FCS among them), `mic_failures`, `quarantined` and
`prefix_other`. The driver keeps no count of the MAC's hardware
buffer-full events. `station` is the station service's receive decisions:
`port_events_lost`, frames `dropped_…` by reason (`replay`, `integrity`,
`security`, …), MSDUs `refused_…` (`too_long`, `queue_full`,
`no_network`) and `copied_…` (`aggregated`, `kept`), the MPDUs the
receive agreements admitted `mpdus_in_ampdu`, `mpdus_single` or
`mpdus_unknown`, the reorder's released MPDUs `released_in_order`,
`released_by_timeout`, `released_by_bar`, `released_by_stop` and skipped
sequences `missing_by_timeout`, `missing_other`, its `duplicates` and
`rejected` input, `agreements_started`, `agreements_stopped`, the latest
agreement's negotiated `agreement_window` at the window's end, and
`rsn_inputs_dropped`.

`tx.ampdu` holds the window's differences of the station's transmit
agreements, also counted only by a trace build: data `aggregates` settled
(by a terminal, or `refused` before the port), their `subframes` and
`average_subframes` (null without an aggregate), the aggregates by
subframes `subframes_1`, `subframes_2`, `subframes_3_4`, `subframes_5_8`,
`subframes_9_16` and `subframes_17_32` (these wrap at 2^16), the members
the BlockAck `acknowledged`, those `retried` in a later aggregate and those
`dropped`, and the `block_ack_requests` the station sent.
Each level's `link`, and the `path` case's `idle link` over the idle
window, record what the link ran, and print one line (`link <level>:`):
the access point's last rate each way and width, the image's commonest
rate each way with its share, its received A-MPDU share and its
transmissions per MPDU. RX and TX stay the image's.

- `router` is the access point's view of the station (the LAB router's
  `station-link` operation, `tools/lab/router`), read before the run's
  start and after its drain (`span_ms` apart): `channel` and `width_mhz` of
  the AP; `to_device` (the image's RX) with the AP's `last_bitrate` to it
  as `iw` printed it and parsed (`text`, `kbit_per_s`, `format`, `mcs`,
  `streams`, `width_mhz`, `short_gi`) and the span's `packets`, `bytes`,
  `retries`, `failed`, `airtime_us` and `tid0_aqm_drops`; `from_device`
  (the image's TX) with its `last_bitrate`, `packets`, `bytes`,
  `airtime_us`, `dropped` and the end's `signal_avg_dbm`; and the in-use
  channel's `survey`: `active_ms`, `busy_ms`, `receive_ms`,
  `bss_receive_ms`, `transmit_ms`, `busy_percent` and the end's
  `noise_dbm`. A count the driver does not report, or one that went back,
  is null; when the router gives no view, `router` is `{"unavailable":
  why}`. A `last_bitrate` is one frame's, at the span's end.
- `device` is the image's count over its window, from its trace build
  (zeros otherwise). `rx`: the data MPDUs addressed to the station that
  reached its port, by their PPDU's rate as the receive metadata decodes
  it (`mpdus`: `total`, every nonzero `rates` entry, e.g. `HT40 SGI MCS7`,
  with its `count` and `percent`, and `ht40_percent`, `short_gi_percent`,
  `mcs7_percent`), and by A-MPDU containment (`in_ampdu`, `single`,
  `containment_unknown`, `in_ampdu_percent`). The metadata marks no
  A-MPDU boundary: `ampdus` and `mpdus_per_ampdu` stay null. `tx`: the
  station's single data MPDUs whose transmission ended, by the rate of
  their last transmission (`mpdus`, as above; one without a transmission
  counts as `unknown`), their `transmissions`, `retries` (transmissions
  beyond one per MPDU that went on air), `transmissions_per_mpdu`, and
  their ends `delivered`, `ack_timeouts`, `other_ends` (protection
  timeout, contention limit, coexistence denial) and `unsent` (expired,
  withdrawn or of unknown outcome). Transmitted aggregates count in the
  level's `tx.ampdu`, not here: `ampdu` is null.

`cache` holds one entry per cache bus the image counts, cleared at the
window's start: `ibus0` (hart 0's instruction fetches through L1-ICache0)
and `ibus1` (hart 1's, which this image does not run), `dbus0` and `dbus1`
(the two data buses of the one L1-DCache; ESP-IDF requests `dbus0` for
every hart's data and never `dbus1`, and which hart's data each carries is
not established). Each has `hits`, `misses`, `conflicts` and
`next_level_reads` (the L1 cache's reads of the L2 cache its accesses
caused), a data bus also `next_level_writes` (write-backs), and
`overflowed`: a 32-bit count of the bus passed its range, the counts are
not exact and every derived value is null. Derived: `miss_percent` (misses
of hits and misses), and per 1000 of the serving hart's (hart 0's)
retired instructions and per frame the IP stack took,
`misses_per_1000_instructions`, `misses_per_stack_frame`,
`next_level_reads_per_…` and a data bus's `next_level_writes_per_…`, to
four decimals; null without the denominator. Only accesses that reach
the L1 cache count. The `path` case also records the `cache geometry` the
image read at its idle window's end: for `l1_icache`, `l1_dcache` and `l2`
the configured `size_bytes` and `line_bytes` (null when the one-hot word
selects no single size), the L1 caches' fixed `ways` (null for the L2,
which no source states), and `l2_bypassed`. With
`OER_WIFI_LOAD_PROFILE_US=1000` (250 to 10000) each level also samples the
image's boot hart (the radio's) every period over the window and records
a `profile`: ticks, stride and effective period, the samples by
interrupted raw CLIC level
(0 task code; with three level bits P1 `0x1f`, P2 `0x3f`, P3 `0x5f`, as
the CPU report's executor levels), the 40
functions and the 40 innermost inlined functions with the most samples, and
every raw sample as `[tick, pc, level, ra]`: `ra` is the interrupted return
address register the image's interrupt entry recorded (null when it
recorded none). A sample inside `memcpy` or `memmove` with its `ra` counts
in the tables as `memcpy ← caller`, the function (and innermost inlined
function) of the call `ra` returns from. Ticks are due every period on the
timer's own schedule; one due while the hart's interrupts were masked
samples where they open. The functions are named from the DWARF of the
runtime ELF the harness built (`target/fw/esp32s31-wifi-station/runtime.elf`,
`oer_analysis_elf::dwarf::Symbolizer`); without it the profile records why
and keeps the raw samples. The profile's `copies` weights the memory copy
call sites the image build found in that ELF (`runtime-copies.json` beside
it, read with `oer_image_checks::copies::read`; without it `copies` records
why): the samples in a copy entry, those whose `ra` is a site's return
address (attributed), those without an `ra` and the rest; and the 40 sites
with the most weight, each with its size where constant, caller, source
chain, the roots that reach it, `in_copy` (samples in the copy that return
to it) and `in_caller` (samples in its caller), with their shares of the
samples. The scenario holds the air exclusively. Executors' work spans and
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
