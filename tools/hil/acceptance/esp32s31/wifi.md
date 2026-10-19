# Wi-Fi acceptance scenarios

HIL catalog outside this repository, state of 2026-10-16; its run records are outside this repository too. Rates are application payload
rates in Mbit/s. RX means toward the ESP32-S31, TX means from it.
"Qualification" marks the scenarios the catalog's qualification requires.

## Peers

- **Lab AP:** the access point the station joins. It is either a managed
  OpenWrt router (mt76; HT20, HT40, HE20 or legacy; WPA2-PSK, SAE or
  SAE-mixed; PMF off, optional or required) or a Linux hostapd AP on the lab
  laptop (WPA2 only).
  - The OpenWrt AP is required for PMF and WPA3, the beacon and DTIM
    schedule, the idle-channel precondition and its own TX monitor.
  - The station tests need an AP the harness can stop and restart.
- **Laptop client:** Linux wpa_supplicant on the lab laptop. It is HT, or
  non-HT when the scenario asks. It is the default client in the AP tests.
- **OpenWrt client:** a managed station interface on the OpenWrt router
  (WPA2-PSK CCMP, or open). Traffic reaches it over wired forwarding.
- **Air monitors:** the OpenWrt AP's TX monitor, and an independent monitor
  (the laptop adapter or a second OpenWrt host on the AP's channel).

## Rules every scenario of a kind applies

### Station UDP

- The session ends with a passing target verdict and no transport error. The
  stack watermarks are consistent and keep headroom.
- **RX floor:** 90 % of the offer unless stated. The host's achieved offer
  and the target's measured rate both meet it.
- **RX radio health:**
  - zero DMA-buffer-full, FIFO-overflow, network-drop, IRQ-drain-saturation
    and unhandled-IRQ counts;
  - the baseband format matches the link (HT or HE);
  - aggregation is HT: hardware A-MPDU, no protocol A-MPDU;
  - aggregation is HE: protocol A-MPDU, no hardware A-MPDU;
  - the reorder state is sane (window 1–64, TID ≤ 7).
- **TX:**
  - No default floor.
  - At least one burst of ≥ 1000 datagrams from sequence 0.
  - At least 100 aggregates prepared, with more subframes than aggregates and
    ≤ 32 subframes each.
  - Zero hardware timeouts and collisions.
  - BlockAck and terminal receipts reconcile.
  - The HT40 link shows 40 MHz MCS7 at the AP.
- **Bidirectional:** both floors default to 90 % of their offers. The
  combined floor applies to target RX + TX.
- **Exact delivery:** sequence 0..N−1 with no gap, reorder or duplicate, and
  byte and unit counts equal on both ends.
- **No beacon loss:** no disconnection for beacon loss on the current boot.
- **Idle channel at most 64/255:** a precondition, sampled on the OpenWrt AP
  for 12 s before the run, so a busy channel is not counted as a device
  failure.
- **Observer-free gates:** the performance image runs without driver
  observers; any driver-internal evidence it publishes fails the run.

### Station TCP

- Exactly one connection per direction, exact bytes, EOF, zero pattern
  errors.
- Without a stated floor the defaults are fixed and ignore the offer: RX 18,
  TX 45; bidirectional RX 9, TX 35.

### Station ICMP

- Up to 3 readiness echoes first.
- Lost replies ≤ the allowed count (default 0).
- Nearest-rank p95 ≤ the stated limit.

### Access-point traffic

- **Per-cycle checks on driver-observation images:**
  - beacons sent, none missed, lateness < 102.4 ms;
  - authentication and association answered, no handshake failure or
    timeout;
  - every client associated, authorized, then removed;
  - zero RX buffer-full and FIFO overflow;
  - zero TX timeouts, MIC failures and quarantined frames;
  - airtime grants reconcile.
- **Multi-client tests:** a per-flow floor, and flow skew
  (max − min) / max ≤ the stated percentage.
- **Laptop plus OpenWrt client without multi-client traffic:** the OpenWrt
  client pings once per second and loses none.

## Station lifecycle and AP availability

### Station reconnect cycles, HE20 (qualification)

- Checks: each cycle disconnects on request and connects at generation + 1,
  then presents complete epoch evidence. Retry exhaustion and hardware or
  contract failures fail the run. Later boots restore the cached startup
  artifact.
- Gate: 2 cold boots × 3 cycles, 90 s each, no beacon loss.
- Peer: lab AP.
- Hardware: pass, stand run 2026-10-11. Earlier: 3 of 3 on
  2026-10-04, 6 of 6 cycles over 2 cold boots on 2026-10-03.

### Station reconnect cycles, HT40 (qualification)

- Checks: as for HE20, on HT40.
- Gate: 10 cold boots × 3 cycles, 90 s each, no beacon loss.
- Peer: lab AP.
- Hardware: pass, stand runs 2026-10-04 and 2026-10-11 (the latter
  on an earlier baseline revision).

### AP loss, reconnect and fresh IP exchange (qualification)

- Checks, in order:
  1. Connected at generation 0.
  2. The AP is stopped.
  3. The station disconnects for beacon loss.
  4. The AP is restarted.
  5. The station connects at generation 1.
  6. Three fresh 32-byte ICMP echoes on a new socket are all answered.
  7. The control interface answers.
- Gate: 3 repetitions. Each step within 120 s; each echo within 2 s; status
  within 3 s.
- Peer: lab AP that the harness can stop (HT40).
- Hardware: pass, stand runs 2026-10-03 (3 of 3) and 2026-10-11.

### AP gone after connection: bounded retries exhausted (qualification)

- Checks: after the connected AP is removed:
  - exactly two NoCandidate attempt failures;
  - then retry exhaustion after 3 attempts, in generation 1;
  - the control interface answers.
- Gate: 120 s; status within 3 s.
- Peer: lab AP that the harness can stop.
- Hardware: pass, stand runs 2026-10-03 and 2026-10-11.

### AP absent from the start (qualification)

- Checks: the AP is stopped before the station starts. The role is admitted
  (Idle to Station), then three candidate attempts in generation 0 exhaust
  without a connection. The control interface stays responsive.
- Gate: 3 repetitions; 3 attempts; 120 s.
- Peer: lab AP that the harness can stop.
- Hardware: pass on 2026-10-11, but only in a stand run on an earlier
  baseline revision of the catalog's stack; not confirmed on its last tree.

### Radio cold restart of a connected HT40/WPA2 station (qualification)

- Checks: in each cycle:
  - the station stops and disconnects by link policy;
  - the radio restarts with RF closed and woken, at stopped generation + 1;
  - the station starts again at generation + 1.

  Before and after every cycle a session-identified UDP exchange runs both
  ways: 4 datagrams to the target, and 1 to 16 back from it at 1 kbit/s, in
  order and exactly reconciled. The link stays 40 MHz WPA2. Radio and network
  work run on different cores.
- Gate: 3 repetitions × 3 cycles, 90 s each.
- Peer: lab AP (HT40, WPA2).
- Hardware: pass (flaky), stand run 2026-10-11, 3 of 3. 13 of 21
  runs failed over its history.

### Station restart against a PMF-required AP

- Checks: PMF negotiated with PSK-SHA256, a protected BlockAck, and UDP before
  and after one station stop and cold restart. The reassociation meets the
  AP's SA Query of the previous association.
- Gate: 1 restart cycle, with the same UDP exchange as the radio restart;
  90 s.
- Peer: OpenWrt AP, HT40, WPA2 with PMF required.
- Hardware: pass, stand runs 2026-09-29 and 2026-10-03.

### Station restart against a WPA3-Personal (SAE-only) AP

- Checks: SAE authentication with protected management frames, and UDP before
  and after one station stop and cold restart.
- Gate: 1 cycle, with the same UDP exchange; 90 s.
- Peer: OpenWrt AP, HT40, SAE only.
- Hardware: pass, stand run 2026-09-29.

## Station roles: scan, monitor, sleep

### Full-band scan

- Checks: a scan of channels 1–13 finds the lab SSID, and the station
  restarts afterwards.
- Gate: 200 ms dwell per channel; the total time is 95–150 % of 2.6 s.
- Peer: lab AP.
- Hardware: pass, stand run 2026-09-29.

### Station to monitor and back

- Checks: after a scan, monitor mode on the SSID's channel captures frames.
  The generation and channel stay the same, with zero mismatches. The station
  is then restored.
- Gate: 3 s dwell, elapsed time 95–150 % of the dwell, more than 0 frames and
  bytes, 256-byte snapshot.
- Peer: lab AP (traffic source on air).
- Hardware: pass, stand run 2026-09-29.

### Station, idle, scan and monitor roundtrip

- Checks: the same sequence on channel 6, with full-length snapshots. It ran
  the same code as station to monitor.
- Gate: 3 s dwell, 95–150 %, more than 0 frames.
- Peer: lab AP.
- Hardware: pass, stand run 2026-09-29.

### Finite monitor capture exported as pcapng

- Checks: the capture keeps channel and generation. Exported frames equal
  captured ones; published frames equal exported plus discarded. Reassembly is
  complete, with no incomplete frame. The station restarts afterwards.
- Gate: 3 s capture, elapsed 95–110 %, more than 0 frames.
- Peer: lab AP.
- Hardware: pass, stand run 2026-10-04. More than 1000 frames
  captured on 2026-09-05.

### Modem sleep: DTIM 1, DTIM 3, and maximum depth (listen interval 10)

- Checks: under power save the RF sleeps, and its sleep and wake edges pair
  up. The MAC local-time counter across the sleep is classified (runs, holds,
  jumps), recorded but not gated.
- Gate:
  - At least one paired RF sleep.
  - ICMP keeps the link busy: 60 echoes every 500 ms (DTIM cases) or 30
    every 2 s (maximum depth). One reply is enough.
  - Beacon interval 100 TU.
- Peer: OpenWrt AP, HE20, which sets the beacon interval and DTIM period.
- Hardware: pass. DTIM 1 on 2026-10-11, DTIM 3 on 2026-10-03, maximum depth on
  2026-10-03 (58 RF sleeps of 941–1021 ms, ICMP 30 of 30).

## Station throughput

Each run lasts 16 s and carries 1472-byte UDP payloads unless stated.
"Repetitions" is the count of runs that must all pass. Observer-free rows run
on the performance image.

| Scenario | Offer | Gate | Hardware |
| --- | --- | --- | --- |
| HT40 UDP RX ceiling, 3 runs | RX 130 | RX ≥ 100, no beacon loss | pass, 2026-10-12, 108.3–112.8 |
| HT40 UDP TX ceiling, 3 runs | TX 130 | TX ≥ 100, no beacon loss | pass, 2026-10-12; 114.2–114.4 on 2026-09-29 |
| HT40 UDP RX production gate, observer-free, 3 runs | RX 130 | RX ≥ 90, idle channel ≤ 64/255 | pass, 2026-09-28, 102.7–102.9 |
| HT40 UDP TX production gate, observer-free, 3 runs | TX 130 | TX ≥ 115, idle channel ≤ 64/255 | pass, 2026-09-28, 119.2–119.4 |
| HT40 UDP RX split-core baseline, observer-free, 3 runs | RX 80 | RX ≥ 72 | not confirmed |
| HT40 UDP TX split-core baseline, observer-free, 3 runs | TX 80 | TX ≥ 72 | not confirmed |
| HT40 UDP RX at 512-byte payloads, observer-free, 12 s | RX 40 | RX ≥ 30, idle channel ≤ 64/255 | not confirmed |
| HE20 UDP RX ceiling, exact delivery, 1200-byte payloads | RX 100 | RX ≥ 90, exact, no beacon loss | not confirmed |
| HE20 UDP TX ceiling, exact delivery | TX 110 | TX ≥ 90, exact, no beacon loss | pass, 2026-09-29, 90.7 |
| HE20 UDP TX sustained, exact delivery | TX 100 | TX ≥ 70, exact, no beacon loss | pass, 2026-09-29, 90.8 |
| HE20 UDP RX ceiling, observer-free, 3 runs | RX 130 | RX ≥ 40 | pass, 2026-09-28, 90.1–90.3 |
| HE20 UDP TX ceiling, observer-free, 3 runs | TX 130 | TX ≥ 40 | not confirmed |
| HT40 full-duplex exact RX delivery with both air observers (qualification), 3 runs | RX 55 + TX 55 | RX ≥ 49.5, TX ≥ 49.5, sum ≥ 100, exact, no beacon loss | not confirmed (never passed on record) |
| HT40 full-duplex split-core baseline, observer-free, 3 runs | RX 40 + TX 40 | each ≥ 35, sum ≥ 72 | not confirmed |
| HT40 full-duplex matrix, 1200-byte payloads, 3 runs each | RX/TX 15/75, 25/65, 35/55, 45/45, 55/35, 65/25, 75/15 | each ≥ 90 % of its offer, no beacon loss | not confirmed |
| HT40 full-duplex five-minute soak, 300 s | RX 5 + TX 5 | each ≥ 4.5, sum ≥ 9, no beacon loss | pass, 2026-09-29, 5.0 + 5.0 |
| HT40 near-idle CPU load, 3 runs | RX 0.1 | RX ≥ 0.09, no beacon loss; CPU load recorded | pass, 2026-10-12; core 0 1.72–1.79 %, core 1 0.09–0.10 % |
| HE20 TCP RX sustained, 32 KiB chunks | RX 80 | RX ≥ 60, no beacon loss | pass, 2026-10-12 (no rate recorded) |
| HE20 TCP TX sustained | TX 70 | TX ≥ 50, no beacon loss | pass, 2026-10-12 (no rate recorded) |
| HE20 TCP both ways | 30 + 30 | each ≥ 25, no beacon loss | pass, 2026-10-09; 2026-10-04: TX 29.99, RX 29.2–29.5 |
| HT40 TCP RX ceiling, 3 runs | RX 150 | RX ≥ 60 | pass, 2026-09-28, 103.0–104.9 |
| HT40 TCP TX ceiling, 3 runs | TX 150 | TX ≥ 65 | pass, 2026-09-28, 88.9–89.2 |
| HT40 TCP RX sustained split-core, 3 runs | RX 80 | RX ≥ 60 | not confirmed |
| HT40 TCP TX sustained split-core, 3 runs | TX 120 | TX ≥ 65 | not confirmed |
| HT40 TCP both ways split-core, 3 runs | 30 + 30 | each ≥ 25 | not confirmed |
| HE20 TCP RX ceiling, 3 runs | RX 130 | RX ≥ 30 | not confirmed |
| HE20 TCP TX ceiling, 3 runs | TX 130 | TX ≥ 30 | not confirmed |

Peer for every row: lab AP; the OpenWrt AP when an idle-channel precondition
or an air observer is named. The host on the AP's wired side is the traffic
endpoint.

## Station latency

| Scenario | Probe | Gate | Hardware |
| --- | --- | --- | --- |
| HE20 connected-station ICMP | 100 × 56 B every 20 ms, 1 s timeout | 0 lost, p95 ≤ 20 ms, no beacon loss | pass, 2026-10-09; 100 of 100, p50 2.17–2.41 ms, p99 2.71–4.64 ms (2026-10-03) |
| HT40 split-core ICMP, 3 runs | same | 0 lost, p95 ≤ 20 ms | pass, 2026-10-04 |
| ICMP with a non-HT (legacy) AP | same | 0 lost, p95 ≤ 20 ms, no beacon loss | fail, 2026-10-04: joins but never reaches Connected |

## Station transmit protection

### Station TX while the AP protects HT PPDUs for a non-HT member

- Checks: the AP advertises HT protection mode 3 in every beacon. The
  station's data PPDUs are preceded inside their NAV by its RTS or by a CTS to
  it. Its RTS uses a basic or mandatory rate. Its NAV covers the AP's
  BlockAck.
- Gate:
  - TX 16 offered; ≥ 15 Mbit/s, exact delivery, no beacon loss.
  - ≥ 95 % of data PPDUs protected. Zero wrong-rate RTS, zero short NAV.
  - At least 50 observed PPDUs, else the observation is insufficient.
- Peer: lab AP (HT40), the laptop associated as a non-HT member, and an
  independent air monitor.
- Hardware: not confirmed.

### Station TX beside an overlapping 802.11b BSS

- Checks: as above, with the AP setting ERP Use_Protection because a legacy
  BSS overlaps. RTS must be DSSS/HR.
- Gate: as above (≥ 15 Mbit/s, ≥ 95 % protected).
- Peer: lab AP (HT40), the laptop running an 802.11b BSS, and an air monitor.
- Hardware: not confirmed.

## PHY maintenance experiment (A/B arms)

Each arm compares station throughput with the shared PHY's periodic tracking
either running (under the vendor admission rule) or suspended for the session.
A suspended session that tracks more than one tick fails. 3 runs each.

| Arm | Offer | Gate | Hardware |
| --- | --- | --- | --- |
| HT40 UDP RX, tracking on | RX 115 | RX ≥ 103.5 (90 %) | pass, 2026-09-29, 113.3–113.8 |
| HT40 UDP RX, tracking suspended | RX 115 | RX ≥ 103.5 (90 %) | pass, 2026-09-29, 113.3–113.8 |
| HT40 UDP TX, tracking on | TX 130 | TX path rules only (no floor) | not confirmed |
| HT40 UDP TX, tracking suspended | TX 130 | TX path rules only (no floor) | not confirmed |

## Access point

The AP is WPA2 unless stated. "Epochs" are AP ownership epochs (start, serve,
stop) within one boot.

### AP cold boot and ownership lifecycle

- Checks: the AP starts, a client associates and the AP stops; this repeats
  across cold boots. The stop evidence matches generation, channel and width.
- Gate: 5 cold boots × 3 epochs, 90 s each.
- Peer: laptop client.
- Hardware: pass, stand run 2026-10-11.

### Two simultaneous WPA2 clients

- Checks: the laptop and the OpenWrt client associate together in each epoch
  and answer ICMP.
- Gate: 2 epochs; 10 echoes every 100 ms; 0 lost; p95 ≤ 50 ms.
- Peer: laptop and OpenWrt clients.
- Hardware: pass, stand run 2026-10-03.

### AP HT40 ICMP latency

- Checks: latency and loss to the client in each WPA2 epoch at HT40 MCS7.
- Gate: 2 epochs × 100 echoes every 20 ms; 0 lost; p95 ≤ 20 ms.
- Peer: OpenWrt client.
- Hardware: pass, stand run 2026-10-05.

### AP throughput

| Scenario | Clients | Offer | Gate | Hardware |
| --- | --- | --- | --- | --- |
| UDP RX, exact, 2 epochs × 10 s, 1200 B | laptop | RX 2 | RX ≥ 1.5, exact | not confirmed |
| UDP TX, exact, 2 epochs × 10 s, 1200 B | laptop | TX 2 | TX ≥ 1.5, exact | not confirmed |
| UDP both ways, exact, 2 epochs × 10 s, 1200 B | laptop | 2 + 2 | each ≥ 1.5, exact | pass, 2026-10-05; 20 runs, all 2084 datagrams delivered (13 failures in a row before the fix) |
| UDP both ways, exact, OpenWrt A/B of the above | OpenWrt | 2 + 2 | each ≥ 1.5, exact | pass, 2026-10-05 |
| TCP RX, 2 epochs × 10 s, 8 KiB chunks | laptop | RX 5 | RX ≥ 4, exact bytes | not confirmed |
| TCP TX | laptop | TX 5 | TX ≥ 4, exact bytes | pass, 2026-10-09 |
| TCP both ways | laptop | 5 + 5 | each ≥ 4 | not confirmed |
| Single client both ways, 16 s, 1400 B | laptop | 4+4, 6+6, 8+8 | each ≥ 95 % of offer, exact | not confirmed |
| Two-client inbound fairness, 16 s, 1400 B | laptop + OpenWrt | RX 16 per flow | RX ≥ 30.4, per flow ≥ 15.2, skew ≤ 20 %, exact | not confirmed |
| Two-client outbound fairness | laptop + OpenWrt | TX 16 per flow | TX ≥ 30.4, per flow ≥ 15.2, skew ≤ 20 %, exact | not confirmed |
| Two-client four-flow balanced | laptop + OpenWrt | 8 + 8 per client | each direction ≥ 15.2, sum ≥ 30.4, per flow ≥ 7.6, skew ≤ 20 %, exact | not confirmed |
| Two-client HT40 TX saturation, exact, 15 s, 1400 B | laptop + OpenWrt | TX 65 per flow | TX ≥ 100, per flow ≥ 45, skew ≤ 20 %, exact | pass, 2026-09-29; about 2440 aggregates per client, up to 32 subframes |
| Single-client sustained RX, observer-free, 1400 B | OpenWrt | RX 16 | RX ≥ 15.2 | not confirmed |
| Single-client sustained TX, observer-free | OpenWrt | TX 16 | TX ≥ 15.2 | not confirmed |
| Single-client balanced, observer-free, 1472 B | OpenWrt | 40 + 40 | each ≥ 35, sum ≥ 72 | not confirmed |
| Single-client HT40 RX ceiling, observer-free | OpenWrt | RX 130 | RX ≥ 95 | pass, 2026-10-12; 109.8–110.0 on 2026-09-28 |
| Single-client HT40 TX ceiling, observer-free | OpenWrt | TX 130 | TX ≥ 95 | pass, 2026-10-12; 121.1–121.2 on 2026-09-28 |
| Single-client HT40 saturated both ways, observer-free | OpenWrt | 65 + 65 | each ≥ 40, sum ≥ 80 | not confirmed |
| Two-client HT40 TX fairness ceiling, observer-free | laptop + OpenWrt | TX 65 per flow | TX ≥ 105, per flow ≥ 48, skew ≤ 15 % | pass, 2026-09-28, 120.5–120.7 (60.2 + 60.4) |
| HT40 TX to a non-HT laptop, observer-free | non-HT laptop | TX 40 | TX ≥ 17 | pass, 2026-09-26, 19.2–19.7 over 6 epochs |
| HT40 saturated peer plus sparse peer, observer-free | laptop + OpenWrt | TX 130 + sparse 4.71 kbit/s in pairs | TX ≥ 105; sparse peer ≥ 8 datagrams, inter-arrival ≤ 5.5 s | not confirmed |

Durations are 16 s and runs use 2 epochs unless stated. Observer-free rows
run on the performance image.

### AP protects HT PPDUs because a non-HT laptop is associated

- Checks: with the non-HT laptop associated, the AP advertises HT protection.
  It protects its HT PPDUs to the OpenWrt client with RTS/CTS, at correct
  control rates and with NAV coverage (the rules of station protection
  above).
- Gate: TX 8 per flow; aggregate ≥ 15, per flow ≥ 7.5, exact; ≥ 95 % of PPDUs
  protected, zero wrong-rate RTS, zero short NAV.
- Peer: non-HT laptop client, OpenWrt client, and a whole-channel air monitor.
- Hardware: pass, stand run 2026-09-28. Aggregate 16.0 Mbit/s;
  97.4 % of PPDUs had RTS/CTS before data.

## Station and access point on one channel

### STA+AP lifecycle (qualification)

- Checks: one production owner runs a station and an AP on the same channel.
  Both IPv4 endpoints come up, the pair holds, and stopping it returns every
  owner (back to Idle, same generation).
- Gate: 5 s dwell, 120 s timeout.
- Peer: lab AP (HT40) upstream; laptop client on the S31's AP.
- Hardware: pass, stand run 2026-10-11.

### STA+AP simultaneous UDP load

- Checks: UDP runs both ways on both interfaces at once. Beacon timing holds
  (beacons sent, none missed, lateness < 102.4 ms), with zero RX buffer-full
  or FIFO overflow. TX has no reorder or duplicate.
- Gate:
  - 15 s, 10 Mbit/s per flow, at least 5 per flow.
  - Interface fairness: max ≤ min × 1.5. HT40 at MCS ≥ 7.
- Peer: lab AP upstream, laptop client on the S31's AP.
- Hardware: not confirmed.

### STA+AP teardown on upstream loss and a fresh pair

- Checks:
  - The pair comes up with the laptop joined.
  - The upstream AP stops, and the station disconnects for beacon loss.
  - The pair stops.
  - The upstream returns, and a second pair (new generation, connected
    generation + 1) comes up with the laptop rejoining.
- Gate: beacon-loss disconnect within 15 s; 120 s overall.
- Peer: lab AP that the harness can stop, plus the laptop client.
- Hardware: not confirmed.

## Left out

Every scenario on a diagnostic image (130 Wi-Fi scenarios) is left out. These
images carry probes, cycle counters, task-poll or task-residence samplers,
MAC-IRQ or TX-wait traces, RX-delivery frontiers, TX-architecture or
DMA-placement variants, an RX clock or interrupt-latency probe, or a PHY
tracking hold. Their floors characterize the instrument, not the product.

Also left out are investigation controls on product images whose only gate is
"some traffic flowed" (≥ 1 bit/s or ≥ 0.1 Mbit/s) or a 10 Mbit/s floor:

- AP airtime, threshold, cold-epoch, SGI and duplex sweeps;
- two-client AP endpoint smoke runs;
- the single-client AP TX correctness control;
- single-core station controls;
- the station 65/65 control;
- a beacon-loss air diagnostic and the RTS/CTS air investigation.

Measured values from these are kept as measurements, not as acceptance.
