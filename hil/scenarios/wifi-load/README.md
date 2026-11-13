# Wi-Fi station load scenario

`cargo hil run wifi-load` measures UDP load between this host and the
product Wi-Fi station and network core of the `hil-wifi-station` image, through the lab
router: host → Ethernet → OpenWrt AP → Wi-Fi → station, one hop over the
air. The host sends and receives the load; the image counts its end through
the [load service](../../services/load) and reports its CPU. Only parameters
and counters cross the HIL link. RX and TX are named from the image's side.

| Case | Does |
| --- | --- |
| `path` | Checks the host's route to the station network (through the router's `[router].lan_address`, over a wired interface, from one source address), has the router bring up a generated WPA2 network that forwards UDP between its stations and that address, joins, takes a DHCP address, and proves both directions with datagrams of the run: the host's probes come back echoed, the image's datagrams reach the host. Records the topology, addresses, MAC, BSSID, channel and route, and the image's CPU and link over a 5 s window without load: the levels' baseline |
| `rx` | The host sends to the image |
| `tx` | The image sends to the host |
| `rx-tx` | Both at once, half the total each way |
| `ping` | The host's own `ping`, 120 requests of 56 bytes 113 ms apart over the idle link, answered by the network core's ICMP echo. Fails above a median of 3 ms; fewer than 30 replies leave it incomplete |
| `combined` | 65 Mbit/s each way at once, 130 in all, RTT probes and a `ping` beside them |
| `overload` | 130 Mbit/s each way at once, more than the air holds either way, RTT probes and a `ping` beside them |
| `sparse` | Each shape both ways at once with a `ping` beside it and no RTT probes: 1 and 2 datagrams every 100 ms, 1 every 1 ms, bursts of 8 every 20 ms and of 32 every 50 ms, and 8 of 200 bytes every 20 ms. Fails above a median ping of 3 ms; a shape's ping of fewer than 30 replies leaves the case incomplete. The ping's 113 ms, a prime number of milliseconds, is no divisor or multiple of any shape's period, so its requests meet each shape at changing phases (13 ms further each in a 20, 50 or 100 ms period), not always at one |
| `stop` | Leaves 2 s into an RX run of 65 Mbit/s and records how long the leave took. Fails unless the station answers `Down`: the network stops and closes the load socket, every lent receive buffer returns by the stop's 2 s deadline, and the radio retires; the run itself ends at its time without the socket. Runs last, because the image serves no station after a leave |

Each load case offers 10, 30, 50 and 65 Mbit/s in total (1472-byte
datagrams); `OER_WIFI_LOAD_MBPS=10,65,100` replaces the levels (and `OER_WIFI_LOAD_WIDTH=ht20` the 40 MHz channel the path case asks the router for), e.g. to
overload. Level 0 means no pacing, in every direction the case has: the host
sends RX load, and the image sends TX load, one datagram after another as
fast as the socket takes them until the window ends (the datagram burst does
not apply). A refused host send is counted in `host_send_failed` and the next
datagram follows at once. Once the window's deadline has passed no new send
starts; a send already started may finish after it. An
unpaced level has no offered rate: its `offered_mbps` and `offered_total_mbps`
are `null`, and its name reads `<case> unpaced`. A level above 0 is paced at
that total, half each way for `rx-tx`. `OER_WIFI_LOAD_WIDTH=he20` asks the router for its HE20 station
profile and the station's join for HE20 (experimental, off otherwise); the
path case prints the PHY the station associated in, and each level's `rx`
and `tx` counts name the HE-SU MPDUs (`HE`). Each level is one run of the image's load service: a 5 s
(`OER_WIFI_LOAD_SECONDS` replaces it, 1 to 60) measuring window after a
start barrier and 300 ms of warm-up, then 500 ms of drain. Start first opens
the device socket. The host starts a continuous incoming flow for RX/mixed;
Begin releases the window only after delivery of this run's first load
datagram (not an Echo or foreign run). TX needs only its already running
host receiver. Begin answers after device preparation actually opens the
window; the controller makes no separate guess at its beginning. The host
flow continues until the device reports the completed window and drain.
A run no Begin released within 10 s of its Start ends without a window:
its Begin and its collections are refused with `NotReleased`. The image
ends the run by itself.

`start_boundary` records Start/Begin call and response times and first/last
successful UDP sends on the host endpoint's monotonic clock, plus the
device's armed, trigger, window and first/last delivery times. The two clocks
are distinct; their absolute timestamps must not be subtracted. The first
device stamp is socket delivery, not DMA completion. An image without the
`load/begin` endpoint cannot serve this controller. Host totals and their loss include warm-up
and drain (and the report polling tail); driver/UDP window differences are
used to assess delivery inside the measured window.

Throughput and CPU are over the window (the image's
clock); per level it records offered, sent and received throughput, lost
(sent less received), loss below the highest sequence, reordered, duplicate
and untracked datagrams (an exact tally, shared by both ends), the sender's
refused and missed sends, RTT of 20 probes per second from Begin's answer
through collection of the finished window
(replies read through the drain) echoed by the image (min, median,
p95, max, replies; `OER_WIFI_LOAD_PROBES=off` sends no probes), each hart's busy share (1 − idle/window: the boot hart's
radio and station, hart 1's network and load) with each executor's work
share, each named part's polls (tasks, the station work's and the network's
futures and the datapath actor's spans: count, rate, share of the window
with an open poll's part, and the mean and longest of the ended polls), and
the window's counters (the boot hart's cycles and retired instructions, their
rate per microsecond and per cycle, the network bridge's taken and refused
frames, the datagrams the network delivered with the hart's instructions over
them, the L1 cache's `cache` counts and the receive path's `rx` counts), and
hart 1's own counters in `hart1`.
The idle baseline records the parts, counters, link and PHY tracking too.

`hart1` holds hart 1's own cycle and retired instruction counters, read on
hart 1 at the window's very ends: `start` and `end`, their differences
`cycles` and `instructions` (null for a counter that went back), `advanced`
(false for one that went back or stood still), `cycles_per_us` and
`instructions_per_cycle`; a load level adds its `datagrams`, those of its
directions within the window, and `instructions_per_datagram`. Every
instruction of the hart counts, its interrupts, the network's and the load
server's own work and the executor's among them: per datagram is a share of
the window, not a path's length. A sampled run counts its sampler too.
`cycles` run through `wfi` at the core clock and `instructions` stand still
there, so the idle baseline's rate is the executor's wakes alone; with a
debugger attached the executor does not wait in `wfi`, and its spin counts.

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
frames `dropped_…` by reason (`replay`, `integrity`,
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

Each level's `tracking`, and the `path` case's `idle tracking`, record
what the image's periodic PHY tracking did over the window, and print one
line (`tracking <level>:`); only a trace image counts it (`available`
false otherwise). The shared RF actor's tracking deadline falls 1 s after
the previous tracking, so a 5 s window holds about five ticks. `ticks`
ran the tracking graph, `not_due` found no client's tracking due and
`unavailable` found the domain unable to track. `branches` counts the
tracked ticks that committed each branch: `inhibited` (the registered
policy skipped every child), `rfpll_corrected` (the RFPLL capacitance
correction), `wifi_i2c_changed` (a Wi-Fi PHY-I2C temperature band
change), `wifi_tx_power` and `bluetooth_ieee802154_tx_power` (a TX power
gain change), `calibration_common` (the RX recalibration) and
`calibration_transmit` (the TX recalibration). `temperature_sensor_units`
holds the PHY's temperature after the window's last tracked tick and its
`min` and `max` over the window, in PHY sensor units: no Celsius
conversion is established. `tick_us` is the ticks' whole time and
`longest_tick_us` the longest.

Each level's `turn_profile` records each network turn's active cycles on
hart 1 against the work it did, and prints one line (`turn profile
<level>:`); only a trace image counts it (`available` false otherwise).
A turn's cycles run from its beginning to its end on hart 1's cycle
counter, interrupts and the trace's own counting included; nothing is kept
per turn. `moments` holds the sums over the window's turns of every
product of two of the variables `one`, `frames` (received frames the turn
took in), `accepted` (frames its port accepted), `notices` (transmit
notices it handed the policy), `requests` (requests it took from the
sockets' lanes) and `cycles`, from which a least-squares fit of cycles to
the four counts follows. `empty` holds the turns with none of the four
(count, cycles, cycles squared), `idle` the turns a wait followed, and
`gaps` the cycles from a turn's end to the next turn's beginning by how
the pass after it went on (`after_yield`, `after_ready_wait`,
`after_pending_wait`, the last with the executor's other tasks and its
sleep). `receive_only`, `transmit_only` and `notice_only` hold the turns
that did one kind of work only, by its count, with their mean cycles.
`cycles_by_log2` counts the turns below 2^9 cycles, then by power of two
to 2^23 and more; `notices_by_turn` and `frames_by_turn` the turns by
their notices and frames; `settling` the turns that handed on a notice,
those of them whose port accepted a frame and the frames accepted in
them. `returns` counts the station's bursts of the network's memory back
through its return queue, each ended by one flush: the memories they
queued, the bursts by size (`queued_by_burst`), the queue's occupancy
before and after each burst, and the bursts that left memory with the
station for want of room (`held_bursts`, `held`).

Each level's `network_turns` records what the network core's service did
in its turns over the window, and prints one line (`network turns
<level>:`); only a trace build counts them (`available` false otherwise). `requests` counts the requests
the turns took from the sockets' lanes by kind, with `reserve_per_send`.
One reservation asks for several blocks: `reserve_blocks` counts the blocks
the reservations asked for, `blocks_per_reserve` their mean and
`reserve_by_size` the reservations by how many (`1`, `2`, `3-4`, `5-8`,
`9-16`).
`budget_spent` counts the sockets whose lane still held requests when the
turn had taken its budget of them, `budget_turns` the turns with any and
`budget_turn_share` their share of `turns`; `intake_full` the turns that
took in their whole budget of received frames. `accepted` counts the frames
the port accepted, `accepted_per_turn` their mean and `accepted_by_turn`
the turns by how many (`0` to `4`, `5-8`, `9+`). `publish_end` counts the
turns by how publishing ended: `chose` (the policy chose nothing),
`refused` (the port had no room or link), `budget` (publishing took its
whole budget), `deferred` (notices were left untold) and `fault`.
`out_at_turn_end` counts the turns that ended with no free `blocks`, and
with an open socket out of `credits`. `yields` counts the passes after a
turn that moved and gave the executor its turn, `yields_with_work` those
after a turn known to leave work (a spent budget, a full intake,
publishing at its budget or deferred); `waits_pending` the waits that
found nothing as they registered, which end the poll unless a deadline
was due, and `waits_ready` those that found something and turned again in
the same poll. Nothing counts which source woke a poll. `producer`
counts a local producer's steps: its offers (`offered`), the core's final
refusals (`refused`) and holds (`held`) of them, declining fills
(`declined`), withdrawals of stale works (`withdrawn`), the waits that
registered it for new offers (`waits_offers`) or for changes only
(`waits_changes`), the wakes the producer itself gave the network's task
(`wakes`, with `producer_wakes_per_offer`) and its datagrams of unknown end
(`unknown`).

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
  `noise_dbm`; `radio`, the radio's own counters that moved, by the
  driver's names (`dot11FCSErrorCount`, `dot11RTSSuccessCount`,
  `dot11RTSFailureCount`, `ba_miss`): the whole radio's, not the station's.
  A count the driver does not report, or one that went back, is null; when the router gives no view, `router` is `{"unavailable":
  why}` with `queues` and `answers`. A `last_bitrate` is one frame's, at the span's end.
  `answers` keeps both readings whole: `start` and `end`, each
  `{"answer": …}` as the router gave it or `{"error": why}` for a read
  that gave none. `queues` is the access point's queues between them
  (`oer_tools_lab_client::protocol::station_link::queue::window`): the
  `association`, each TID's and the radio's changes (`valid` with its unit,
  scope and times, `unavailable`, or `invalid` with its reason such as
  `clock-went-back`, `reassociated` or `went-back`) and the `balance`. When
  a read gave no answer or its answer is not the client's station link (an
  older LAB service's), `queues` is `{"unavailable": [why, …]}`, one reason
  per read, and no value is made up.
- `device` is the image's count over its window, from its trace build
  (zeros otherwise). `rx`: the data MPDUs addressed to the station that
  reached its port, by their PPDU's rate as the receive metadata decodes
  it (`mpdus`: `total`, every nonzero `rates` entry, e.g. `HT40 SGI MCS7`,
  with its `count` and `percent`, and `ht40_percent`, `short_gi_percent`,
  `mcs7_percent`), and by A-MPDU containment (`in_ampdu`, `single`,
  `containment_unknown`, `in_ampdu_percent`). The metadata marks no
  A-MPDU boundary: `ampdus` and `mpdus_per_ampdu` stay null. `tx`: the
  station's data MPDUs sent, by rate (`mpdus`, as above): a single MPDU
  by its last transmission's rate (one without a transmission counts as
  `unknown`), an aggregate's subframes by the rate of the aggregate's
  last transmission; then the single MPDUs' `transmissions`, `retries`
  (transmissions beyond one per single MPDU that went on air),
  `transmissions_per_single`, and their ends `delivered`, `ack_timeouts`,
  `other_ends` (protection timeout, contention limit, coexistence denial)
  and `unsent` (expired, withdrawn or of unknown outcome). The aggregates
  themselves count in the level's `tx.ampdu`: `ampdu` here is null.
  `aggregates_missing` counts the aggregates that went by the members
  their BlockAck left missing: `none`, `one`, `two`, `three_four`,
  `five_eight`, `nine_up` with some acknowledged, and `all`, a lost
  aggregate or BlockAck.
  Each level also prints one `link` line: the router's channel and width,
  its last rates each way, and each way the image's MPDUs with every rate
  they went at (MCS, 20 or 40 MHz, short or long guard interval) and its
  share.

`mac` holds the MAC's own statistics that moved over the window, by the
names of the service's `MAC_COUNT_NAMES`: the decoder's errors, the
responses it counted and `frequency_offset` at the window's end. The
image samples the bank every 50 ms from the port's turn, so a window's
ends lag by up to that much.

`stall` localizes where reception last moved, by boundary, and prints one
line (`stall <level>:`). Times are milliseconds from the image's window
start (`window_ms` its length), on the image's clock. `last_progress_ms`
holds the DMA boundary's (descriptors of completed units a receive service
pass observed), the driver's (units the radio device handed to the station)
and the socket's (load datagrams the load server received). After the
window's own readings the image asks its radio owner for a snapshot
(`requested_ms`), which the owner takes at the turn the request wakes,
before that turn's receive service: `snapshot` holds the device's own ring
work (`rx_pending`, a `continuation`, a `cut`, staged units `queued`), the
walker's registers (enable, reload doorbell, NEXT, LAST, BASE, the MAC's
buffer-full count and runtime state), the ring's view, the descriptors by
their first word (`armed`, `done`, with `software`) and the reserved one,
the released, detached and staged buffers, and the software side of the
interrupt line (a request its handler has not applied, a receive fact not
taken yet, handler entries, unmasks and service passes). The MAC's interrupt
registers belong to the radio handler and are not read. `resume` holds the
boundaries at the owner's facts after the turn's receive pull
(`after_receive_pull` false when none ran within the image's 200 ms wait):
a boundary `resumed` when it moved after the snapshot; the socket's is its
first datagram after the request. `mac_since_periodic_sample` holds what
the MAC counted between its last 50 ms sample and the snapshot's own
reading of the bank.

Each level's `addba`, and the `path` case's `idle addba`, record the
ADDBA Requests the station took since boot, read at the window's end, and
print one line (`addba <level>:`); only a trace image counts them. `last`
holds each TID's last request: the Buffer Size the access point asked for
as received (`requested`, zero asking the station to choose) and its
`outcome`: `accepted` with the agreement's `window`, `repeated` (the
running agreement's own request again) with its `window`, `refused` with
the IEEE 802.11 `status` code of the response put in the queue, or
`unanswered` (no reply place or bank remained; the access point asks
again). `accepted`, `repeated`, `refused` and `unanswered` count the
requests by outcome. Records last from boot, so after a new association a
TID's row may still hold a request of the previous one.

`positions` holds the aggregates a BlockAck answered at their first
transmission, by position on air, the first one at index 0; a lost
aggregate or BlockAck and a retransmission count in none of it. `sent` has
the subframes that went at each position, `holes` those left
unacknowledged, `tail_runs` the aggregates whose last subframes went
missing together by the length of that run, a run of one at index 0, and
`interior` the aggregates with a hole whose last subframe was acknowledged.

`cache` holds one entry per cache bus the image counts, cleared at the
window's start: `ibus0` (hart 0's instruction fetches through L1-ICache0)
and `ibus1` (hart 1's), `dbus0` and `dbus1`
(the two data buses of the one L1-DCache; ESP-IDF requests `dbus0` for
every hart's data and never `dbus1`, and which hart's data each carries is
not established). Each has `hits`, `misses`, `conflicts` and
`next_level_reads` (the L1 cache's reads of the L2 cache its accesses
caused), a data bus also `next_level_writes` (write-backs), and
`overflowed`: a 32-bit count of the bus passed its range, the counts are
not exact and every derived value is null. Derived: `miss_percent` (misses
of hits and misses), and per 1000 of the serving hart's (hart 0's)
retired instructions and per datagram the network delivered,
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
runtime ELF the harness built (`target/fw/hil-wifi-station/runtime.elf`,
`oer_tools_analysis_elf::dwarf::Symbolizer`); without it the profile records why
and keeps the raw samples. The profile's `copies` weights the memory copy
call sites the image build found in that ELF (`runtime-copies.json` beside
it, read with `oer_tools_image_checks::copies::read`; without it `copies` records
why): the samples in a copy entry, those whose `ra` is a site's return
address (attributed), those without an `ra` and the rest; and the 40 sites
with the most weight, each with its size where constant, caller, source
chain, the roots that reach it, `in_copy` (samples in the copy that return
to it) and `in_caller` (samples in its caller), with their shares of the
samples. The scenario holds the air exclusively. Executors' work spans and
parts' polls include preemption and overlap: only a hart's idle time gives
its utilisation. The router forwards the load: its CPU can limit it.

`OER_WIFI_LOAD_TUNES=return-quiet=0;return-quiet=16,recycled-burst=1` runs
each level once per `;`-separated set: the values go to the image at the
run's start (`oer_portable_trace::tune`), so comparing them takes one build
and one boot, with the code in one place. An empty set keeps the defaults;
the level's name carries its set. Only a trace image reads them.

A level's `ping`, in the cases that run one, has the shape of its `rtt`.
Apart from the median ping of `ping` and `sparse`, the measurements decide
nothing: a case fails only when the path or an exchange does not work. Each measurement is printed, and after the lease's
release the run's results, its cleanup and verdict go to
`target/hil/results/wifi-load-<time in ns>.json`.

`OER_WIFI_LOAD_VENDOR=socket|fast` runs the RX levels against the vendor
receiver instead of the cases: the stand's ESP-IDF
[UDP receiver peer](../../../tools/lab/peers/esp32s31-wifi-udp-rx/README.md)
in that variant (`cargo fw idf build wifi-udp-rx-peer` or
`wifi-udp-rx-fast-peer` first), on the board `--board` names (or the one
ESP32-S31 board with the `dut` role), under a lease of the board, the router
and the air exclusively. `--list` and `--case path|rx` work as for the cases
and are read, with the environment, before anything reaches the stand;
`rx` joins first. The program writes the peer unless the board runs it,
applies the same station profile with a generated network and hands it to
the peer on its console, then offers each level as `rx` does
(`OER_WIFI_LOAD_MBPS`, `OER_WIFI_LOAD_SECONDS`, `OER_WIFI_LOAD_PROBES` and
`OER_WIFI_LOAD_WIDTH` apply). Each level records the same `rx` counts by the
load header, `rtt` and the access point's `link.router` view, and in `vendor`
the peer's report of that run: its counts, its IP stack's and driver's
counters over the window (`window`, snapshots at the window's start and end,
as the image's device counts) and over the run (`run`, from its RX command
to its drain's end, the span of the access point's readings), the MAC's
counters over the window, and `balance`, the access point's MPDUs to the
station against the driver's count over the run, with what it leaves
`unclosed`, with no place of loss named. The peer's report is measured whole
or not at all: every line the protocol names for the run once and of that
run, nothing else, each with exactly its fields, counts unsigned, the window
and the run's boundaries as the host asked, and the counts consistent with
each other and with what the host sent (no more distinct datagrams or bytes
than it sent, every byte in its datagrams, no sequence it did not send)
(`src/vendor/report.rs`); anything else ends the run as incomplete. The loss
is the host's datagrams less the peer's distinct ones, exact (`lost`) only
when the peer tracked every arrival; with untracked arrivals `lost` is null
and `lost_at_most` bounds it. The peer's link state machine is tested on the
host by `tests/peer_link.rs`, which needs a C compiler (`$OER_HOST_CC`, else
`cc`) and fails without one. Its results go to
`target/hil/results/wifi-load-vendor-<time in ns>.json`.

The host needs, once, a route to the station network through the router's
LAN address: `ip route add 10.231.0.0/24 via <router LAN address>` (or the
same as a permanent route of the wired connection), and the stand file names
that address as `[router].lan_address`.

```console
cargo test -p oer-hil-scenarios-wifi-load
```
