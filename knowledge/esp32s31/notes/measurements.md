# ESP32-S31 hardware measurements of the predecessor

Values the predecessor project recorded on hardware and kept only in its
issue discussions and commit messages. They are data for re-measurement and
for sizing new work, not readiness claims: the code that produced them is not
in this repository, and none of them has been repeated here.

**Status.** *Measured*: the source reports an on-chip run and its result.
*Claimed*: the source states the value without the run, or reports it from
older material it did not rerun. *Withdrawn* and *open* say what the source
later said about it.

**Common conditions** unless a row says otherwise:
- DUT: ESP32-S31 revision 0, stand board s31-a (CoreBoard-1, 16 MB PSRAM,
  16 MB flash, no 32 kHz crystal); every run used this one board.
- Firmware: the predecessor's own Rust stack, PSRAM at 250 MHz, CPU at
  320 MHz where stated.
- Wi-Fi peer: the stand's OpenWrt access point (a laptop as non-HT client
  where stated); station ceiling scenarios use 1472-byte UDP payloads, 16 s
  windows and 3 repetitions.
- Bluetooth LE peer: the stand's Linux BlueZ adapter.
- IEEE 802.15.4 and Direct Test Mode peer: an ESP32-C5 (stand board c5-a)
  running Espressif's vendor firmware or vendor OpenThread.
- The stand sits in shared air: neighbouring Wi-Fi networks and foreign BLE
  scanners are present and not controlled.

## Wi-Fi

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| Station UDP RX 109–113 Mbit/s | HT40, offer 130 Mbit/s, no beacon loss allowed | 2026-09-28 | predecessor commit recording the RX ceilings | measured |
| Station UDP RX ~90 Mbit/s | HE20; at a 90 Mbit/s offer the ceiling equalled the offer, so the offer was raised to 130 Mbit/s | 2026-09-28 | same commit and the commit raising the HE20 offer | measured |
| Station UDP TX ~119 Mbit/s, no beacon loss | HT40, after the control step stopped discarding received beacons | 2026-09-28 | predecessor commit recording the TX ceiling | measured |
| Station TCP RX 103–105 Mbit/s | HT40, the predecessor's owned Xarxa network stack | 2026-09-28 | predecessor commit recording the TCP ceiling | measured |
| UDP RX 113.3 → 113.5, UDP TX 115.0 → 115.1, TCP RX 104.1 → 103.4, TCP TX 89.2 → 89.9 Mbit/s | HT40 station ceilings, A/B of a network-stack pin change, 3 layout seeds × 3 repetitions per arm, all differences within noise | 2026-09-28 | predecessor commit adopting the upstream-merged network forks | measured |
| AP two-client UDP TX 120.1–120.2 Mbit/s (60.1 + 60.1) | AP HT40, two clients; ~2440 A-MPDUs per client, maximum 32 subframes, 99.9 % full aggregates | 2026-09-28 | same commit | measured |
| Station UDP RX 110.6–113.1 Mbit/s (saturated task-residence), 107.8–110.2 Mbit/s (single-core task-residence) | HT40, A/B of a network-stack pin, one repetition per arm and round; air-limited near 110 Mbit/s | 2026-09-28 | predecessor commit pinning the network stack | measured |
| Single-core HT40 RX loses ~46 % of datagrams in bursts at a 140 Mbit/s offer | single-core RX window, saturated offer | 2026-09-28 | predecessor commit moving the profiled offer to 65 Mbit/s | measured |
| HT40 RX 105.07–109.9 Mbit/s at a 120 Mbit/s offer | PHY-maintenance RX arms; run-to-run noise of the ceiling | 2026-09-28 | predecessor commit lowering the maintenance arms' floor | measured |
| RX path in internal SRAM: 96.5 ± 8.1 vs 101.2 ± 0.3 Mbit/s in PSRAM | 3-seed A/B, single-core task-residence scenario; the earlier +15 % came from a single slow PSRAM layout; cost 11.2 KiB SRAM | 2026-09-29 | predecessor revert of the SRAM placement | measured: no gain |
| TCP TX 29.99 Mbit/s both arms; TCP RX 29.49 vs 29.23 ± 2.4 Mbit/s | bidirectional TCP, A/B of a Block Ack refactor | 2026-10-04 | predecessor commit on the portable TX Block Ack originator | measured |
| UDP TX 122.1 and 122.8 Mbit/s, zero loss | channel 13 HT40, 16 s UDP TX; MAC IRQ service average 14.88 µs, maximum 31 µs; 5253 TX and 198 RX MAC interrupt entries | 2026-09-06 | predecessor commit establishing the original network baseline | measured |
| UDP TX 16.083 Mbit/s at a 16 Mbit/s offer, zero loss | same baseline, low offer | 2026-09-06 | same commit | measured |
| HE20 TX 114.7 Mbit/s with complete UDP delivery, 20 MHz width confirmed | HE20 profile acceptance | 2026-09-08 | predecessor commit accepting OpenWrt PHY profiles | measured |
| RTS/CTS on every full A-MPDU: ~330 µs extra per exchange, partial BlockAcks 5 → 69, AP 117 → ~100 Mbit/s | saturated AP, when the 2346-byte RTS threshold was applied to the whole A-MPDU (a defect; the vendor A-MPDU path never applies it) | 2026-09-26 | predecessor commit keeping the RTS threshold off A-MPDUs | measured |
| AP spaced PPDUs ~2.65 ms apart, non-HT client capped near 4.4 Mbit/s | AP with one HT peer under a Block Ack agreement and one non-HT laptop; a 2 ms collection deadline applied to every destination (a defect) | 2026-09-26 | predecessor commits collecting AP TX batches per destination | measured |
| AP TX to a non-HT laptop 19.2–19.7 Mbit/s | single ERP-OFDM MPDUs, six cycles | 2026-09-26 | predecessor commit gating AP transmit to a non-HT client | measured |
| Legacy (non-HT) association completed the WPA2 handshake, then the station never reported Connected | OpenWrt AP in non-HT mode | 2026-10-04 | predecessor issue on legacy association | measured symptom; cause unconfirmed |
| HE20: all 108 742 datagrams at ~80 Mbit/s, but publication/BlockAck counter reconciliation failed | early HE20 run | 2026-09-08 | predecessor facade-reorganization commit | measured, superseded |
| Observer perturbation ~3.7–4.1 % (100.3 vs 96.6 Mbit/s, same image) | older egress experiments, observer off vs on | 2026-09 (older snapshots) | predecessor research issue on S31 egress controls | claimed: reported from snapshots, not rerun |
| Cached PSRAM (0x50xx_xxxx) addresses as Wi-Fi DMA buffers delivered no UDP; same image with SRAM delivered 122.35–122.40 Mbit/s | older egress experiments | 2026-09 (older snapshots) | same issue | claimed; the DMA contract stays SRAM |
| One peer ~124 Mbit/s with early SRAM admission; two peers ~97 Mbit/s at 67 slots, ~121 at 98 slots (+52 576 bytes) | older AP queue-geometry experiments | 2026-09 (older snapshots) | same issue | claimed |
| GDMA PSRAM→SRAM copy of 32 frames: fewer instructions but 2.27× wall time | older copy experiments | 2026-09 (older snapshots) | same issue | claimed |
| Monitor mode received over 1000 frames | standalone monitor example, staged PSRAM boot, PSRAM at 250 MHz | 2026-09-05 | predecessor commit sharing the staged boot between HIL and examples | measured |
| Monitor capture scenario passed (with ICMP latency, station reconnect and AP ICMP) | Wi-Fi interrupt sources enabled through the image's interrupt table | 2026-10-04 | predecessor commit routing Wi-Fi interrupts through the image table | measured (pass, no numbers) |

### Power save and time

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| 353 RF sleeps of 41–304 ms; MAC local-time counter and monotonic time agree within 1 µs | station modem sleep (min-modem), DTIM 1 and 3 set on the AP | 2026-10-03 | predecessor commit citing the station sleep run | measured: the MAC counter runs through RF sleep |
| 256 RF sleeps of 41–99 ms, agreement within 1 µs | max-modem, listen interval 10; max-modem woke for every beacon, so longer sleeps stay unmeasured | 2026-10-03 | predecessor commit on max-modem sleeps | measured |

## Bluetooth LE

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| DTM LE 1M 401 packets, LE 2M 400 → 798 packets received by the peer | ESP32-C5 vendor Controller counting; RF channel 19; LE 2M at 400 was half the vendor's rate because each second 625 µs slot was skipped (a 37-octet LE 2M packet leaves 433 µs to the next slot) | 2026-09-29 | predecessor commit fixing DTM transmitter slots | measured |
| DTM RX on LE 1M, 2M, Coded S=8 and S=2 and TX on LE 1M and 2M against the C5 peer, at least 100 packets per case | RF channel 19, with an idle-transmitter silence control | 2026-09-28 | predecessor scenario definition and qualification commit | measured (pass); per-case counts not recorded in the sources |
| DTM to the Linux adapter: 80 packets delivered, zero false packets while silent, 200 Test End cycles and 100 Reset restarts | LE 1M; the run stopped at the adapter's own LE Test End timeout | 2026-09-27 | predecessor commit on the DTM PHY ETM route | measured |
| High-duty-cycle directed advertising ended with Advertising Timeout after 1.28 s; scan response read by an active Linux scanner; low-duty directed advertising connected | Linux adapter | 2026-09-28 | predecessor commit recording scannable and directed advertising | measured |
| GATT 6 of 9, secure GATT 1 of 9 passed | Trouble GATT against BlueZ, before the cause was found | 2026-10-09 | predecessor issue on intermittent GATT failures | measured |
| CONNECT_IND lost in 22 of 128 attempts (~17 %); foreign SCAN_REQ to ~21 % of ADV_IND (38 of 184 events); 4 of 33 lost with the owner's phone Bluetooth off, 3 of the 4 with a foreign SCAN_REQ in the same event | advertising on channel 37 only; foreign active scanners with rotating random addresses; host and DUT traces aligned within 0.2 ms | 2026-10-09 | same issue | measured: cause is the foreign scanner collision; whether the DUT also loses CONNECT_IND without a collision is open |
| 140 established connections, 0 lost by supervision timeout before the first ATT exchange (< ~2 % at 95 % confidence); not established 11 of 71, 1 of 31, 0 of 50 attempts | GATT 20 reps, traced GATT 10 reps, secure GATT 10 reps | 2026-10-11 | predecessor issue on early supervision timeouts | measured |
| One connection lost by supervision timeout (0x08) ~449 ms after creation, before any ATT exchange | interval 45 ms, supervision timeout 420 ms, the DUT had received the CONNECT_IND | 2026-10-09 | same issue | measured, single event; cause open |
| 22 of 22 repetitions passed (GATT 3, traced GATT 10, secure GATT 9); the fixture retried 2 of 11, 9 of 39 and 6 of 51 attempts the DUT never received | several connections held by the Controller core | 2026-10-11 | predecessor commit on multi-connection Controller | measured |
| 18 of 20 passed (one secure repetition exhausted its three attempts, one GATT repetition lost the link by 0x08) | GATT and secure GATT, 10 + 10 | 2026-10-09 | predecessor issue requiring 10 of 10 | measured; 10 of 10 never confirmed |
| Port poisoned in 1 of 79 secure GATT repetitions | joined BLE driver with lock-free request exchange; cause not reported by that build | 2026-10-12 | predecessor issue on a rare poisoned port | measured; cause unknown |
| Secure GATT 3 of 3; missing-key, key-refresh, wrong-key and ACL backpressure 3 of 3 each; missing-key termination reasons matched the vendor on the same board | Numeric Comparison pairing, encrypted ACL | 2026-09-17, 2026-09-20 | predecessor commits on secure GATT and encrypted ACL | measured |

## IEEE 802.15.4

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| Energy -87 dBm baseline → -41/-42 dBm during the peer's burst, matching the frames' -42 dBm RSSI; -90 dBm on channel 25; -40 dBm after retuning back; -87/-86 dBm after the burst; 3 of 4 CCA-first transmissions found the channel busy | peer burst on channel 15, two boots | 2026-09-27 | predecessor commit recording the channel-energy cell | measured |
| Live RSSI within 2 dB of the last frame's RSSI; followed a 36 dB step | peer transmit power 20 and -15 dBm | 2026-09-28 | predecessor commit requiring the live-RSSI cell | measured |
| Acknowledged exchange, filtering, frame pending and enhanced ACK all passed | DUT at the PIB default 21 dBm, CSMA-CA with retries, two boots | 2026-09-28 | predecessor commit recording the peer exchange | measured |
| ACK loss 7 of 20 repetitions, then 95 clean runs including 20 at the same image | peer sends 8 acknowledged frames ~2.5 ms apart; the DUT missed one frame for all three peer retries, without fault; failures clustered in one hour while the peer's USB console was wedging | 2026-10-03/04 | predecessor research issue on intermittent ACK loss | measured; no code regression established; layout/PSRAM handler hypothesis not supported |
| ED rejected as a coexistence reject with MODEM_LPCON.CLK_CONF = 0x00000001 (only the Wi-Fi power clock open, coexistence and analog-I2C gates closed) | ED probe whose helper dropped the platform clock references | 2026-10-11 | predecessor issue on the ED coexistence reject | measured: closed coexistence clock causes the reject |
| Thread datagram exchange passed | C5 vendor OpenThread peer | 2026-10-11 | predecessor revalidation issue | measured |

## Coexistence

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| Wi-Fi UDP RX with a BLE echo passed | joint Wi-Fi and BLE echo | 2026-09-28, 2026-10-04 | predecessor commits recording the BLE suite and the interrupt-table move | measured (pass, no numbers) |
| 802.15.4 peer exchange passed with the device taking part in Wi-Fi coexistence | | 2026-09-27 | predecessor commit recording the coexistence exchange | measured (pass, no numbers) |
| Wi-Fi works after a full common-power cycle only when the RF wake and the coexistence enable both precede the repeated power entry; the reverse order failed 3 of 3 (station rejoin starves, watchdog reset) | `wifi-radio-restart`; skipping the baseband reset pulse or re-applying the entry's modem and PHY clock writes did not help (3 experiments) | 2026-10-11/12 | predecessor research issue on the power-entry order | measured; which write conflicts is open (candidates: the platform clock references, the cold-baseline capture) |

## PHY

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| Tracking tick RF hold: RFPLL < 1 ms, common bracket ~6 ms idle and up to 13–18 ms under traffic, transmit bracket ~7 ms per class, worst whole tick 30.1 ms | forced tracking branches; full table in the [PHY README](../../../crates/esp32s31/phy/README.md#measured-rf-hold-of-a-tracking-tick) | 2026-10-11/12 | predecessor tracking-hold runs and their review | measured |
| Cold and restart calibration match the vendor except temperature references +2 °C (up to 2–3 °C, beyond the margin of one), COEX_LP_CLK_CONF (0x314 vs 0 after a Wi-Fi restart), live SAR/DC spread and a one-LSB RFPLL spread | 10-boot cross-check against vendor firmware on the same board | 2026-09-28/29, 2026-10-11 | predecessor commits on platform clocks and the analog model; issue on the restart cross-check | measured |
| 802.15.4 calibration cross-check: 191 of 192 registers and 37 of 39 analog registers matched (another run 192/193, 38/39); each difference about one LSB outside the vendor's 5-boot spread | 5 boots each, production vs vendor | 2026-10-11 | predecessor issue on the 802.15.4 cross-check | measured; whether the 5-boot criterion is too tight or a regression is open |
| Bluetooth/802.15.4 TX roughly 12 dB above the requested power after RF wake without a gain republish | older diagnostic branch | 2026-10-06 (older branch) | predecessor research issue on gain-bank retention | claimed: not reproduced; RSSI alone does not establish it |
| RFPLL fresh-sample maintenance skipped as expected; forced maintenance completed with zero correction; 65 Mbit/s HT40 RX without UDP loss | channel 13 HT40 | 2026-09-10 | predecessor commit aligning PHY tracking | measured |

## Interrupt latency and CPU load

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| FROM_CPU raise → handler: idle min 2.03, median 2.04, p99 2.08–2.09, max 2.14 µs; under 59.9 Mbit/s HT40 UDP RX min 2.05, median 2.08, p99 2.13–2.15, max 4.74 µs | core 0, Priority1 (the MAC interrupts' level), CPU 320 MHz, 3 × 1024 samples per load spread over the traffic window | 2026-10-12 | predecessor research issue on interrupt-driven backends | measured; an earlier "under traffic" row was taken before traffic started and was withdrawn |
| CPU load idle link: core 0 1.71–1.79 %, core 1 0.09–0.10 % | 100 kbit/s RX | 2026-10-12 | predecessor PR adding per-core CPU load | measured |
| CPU load at UDP RX ceiling 108.3–112.8 Mbit/s: core 0 54.2–55.3 %, core 1 32.6–33.9 %; on later builds 68.3–68.4 % / 47.5–47.6 % at unchanged throughput | HT40 | 2026-10-12 | same PR | measured; the rise across builds was not bisected |
| CPU load core 0 / core 1: UDP TX ceiling 60.0–60.2 / 41.7–41.8 %; TCP RX 83.3 / 63.5 %; TCP TX 81.2 / 69.6 %; AP single-client RX 82.5 / 57.0 %; AP single-client TX 62.9 / 28.7 % | HT40 | 2026-10-12 | same PR | measured |
| The CPU-load instrument changes no throughput: UDP RX +0.04 ± 0.40, UDP TX -0.13 ± 0.51 Mbit/s | 5 A/B pairs | 2026-10-12 | same PR | measured |

## Power and brownout

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| Brownout reset (reset code 0x0f) about 2 s after the 802.15.4 air check starts, on every run; the board re-enumerates ~0.2 s later | independent of cable, USB port (powered hub) and detector policy (bootloader mode 1, and ESP-IDF's application policy mode 0 level 7); peer exchange, maintenance, Thread and BLE scenarios pass on the same board | 2026-10-10/11 | predecessor issues on the air check and its revalidation | measured: the supply droop is real; which step draws the current (maximum-power TX, ED/CCA, start-up tracking) is open |
| 802.15.4 peer exchange brownouts on the clean baseline | | 2026-09-28 | predecessor commit on the shared 160 MHz and analog-I2C gates | measured, earlier occurrence |
| Reset cause reads 1 (power-on) after a hub power cycle and 0x18 after a JTAG reset, on s31-a and c5-a | read through JTAG | 2026-10-09 | predecessor stand commit proving port power cuts | measured |

## Not hardware measurements

Two predecessor issues on this list carry no on-chip value: the beacon
receive-time and hardware beacon monitor registers (meaning not established)
and the second station group-key slot (the 25-entry key table is a
register-model fact; the receive-side lookup by key id is unproven).
