# ESP32-S31 hardware measurements

Hardware observations identify their source and conditions per row. Current
repository runs retain HIL evidence; older records can live outside this
repository. These figures support sizing and comparison, not readiness claims.

**Status.** *Measured*: the source reports an on-chip run and its result.
*Claimed*: the source states the value without the run, or reports it from
older material it did not rerun. *Withdrawn* and *open* say what the source
later said about it.

**Common conditions** unless a row says otherwise:
- DUT: ESP32-S31 revision 0, stand board s31-a (CoreBoard-1, 16 MB PSRAM,
  16 MB flash, no 32 kHz crystal); every run used this one board.
- Firmware: a Rust stack outside this repository, PSRAM at 250 MHz, CPU at
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
| Station UDP RX 109–113 Mbit/s | HT40, offer 130 Mbit/s, no beacon loss allowed | 2026-09-28 | stand measurement (record outside this repository) | measured |
| Station UDP RX ~90 Mbit/s | HE20; at a 90 Mbit/s offer the ceiling equalled the offer, so the offer was raised to 130 Mbit/s | 2026-09-28 | same record | measured |
| Station UDP TX ~119 Mbit/s, no beacon loss | HT40, after the control step stopped discarding received beacons | 2026-09-28 | stand measurement (record outside this repository) | measured |
| Station TCP RX 103–105 Mbit/s | HT40, an owned Xarxa network stack | 2026-09-28 | stand measurement (record outside this repository) | measured |
| UDP RX 113.3 → 113.5, UDP TX 115.0 → 115.1, TCP RX 104.1 → 103.4, TCP TX 89.2 → 89.9 Mbit/s | HT40 station ceilings, A/B of a network-stack pin change, 3 layout seeds × 3 repetitions per arm, all differences within noise | 2026-09-28 | stand measurement (record outside this repository) | measured |
| AP two-client UDP TX 120.1–120.2 Mbit/s (60.1 + 60.1) | AP HT40, two clients; ~2440 A-MPDUs per client, maximum 32 subframes, 99.9 % full aggregates | 2026-09-28 | same record | measured |
| Station UDP RX 110.6–113.1 Mbit/s (saturated task-residence), 107.8–110.2 Mbit/s (single-core task-residence) | HT40, A/B of a network-stack pin, one repetition per arm and round; air-limited near 110 Mbit/s | 2026-09-28 | stand measurement (record outside this repository) | measured |
| Single-core HT40 RX loses ~46 % of datagrams in bursts at a 140 Mbit/s offer | single-core RX window, saturated offer | 2026-09-28 | stand measurement (record outside this repository) | measured |
| HT40 RX 105.07–109.9 Mbit/s at a 120 Mbit/s offer | PHY-maintenance RX arms; run-to-run noise of the ceiling | 2026-09-28 | stand measurement (record outside this repository) | measured |
| RX path in internal SRAM: 96.5 ± 8.1 vs 101.2 ± 0.3 Mbit/s in PSRAM | 3-seed A/B, single-core task-residence scenario; the earlier +15 % came from a single slow PSRAM layout; cost 11.2 KiB SRAM | 2026-09-29 | stand measurement (record outside this repository) | measured: no gain |
| TCP TX 29.99 Mbit/s both arms; TCP RX 29.49 vs 29.23 ± 2.4 Mbit/s | bidirectional TCP, A/B of a Block Ack refactor | 2026-10-04 | stand measurement (record outside this repository) | measured |
| UDP TX 122.1 and 122.8 Mbit/s, zero loss | channel 13 HT40, 16 s UDP TX; MAC IRQ service average 14.88 µs, maximum 31 µs; 5253 TX and 198 RX MAC interrupt entries | 2026-09-06 | stand measurement (record outside this repository) | measured |
| UDP TX 16.083 Mbit/s at a 16 Mbit/s offer, zero loss | same baseline, low offer | 2026-09-06 | same record | measured |
| HE20 TX 114.7 Mbit/s with complete UDP delivery, 20 MHz width confirmed | HE20 profile acceptance | 2026-09-08 | stand measurement (record outside this repository) | measured |
| RTS/CTS on every full A-MPDU: ~330 µs extra per exchange, partial BlockAcks 5 → 69, AP 117 → ~100 Mbit/s | saturated AP, when the 2346-byte RTS threshold was applied to the whole A-MPDU (a defect; the vendor A-MPDU path never applies it) | 2026-09-26 | stand measurement (record outside this repository) | measured |
| AP spaced PPDUs ~2.65 ms apart, non-HT client capped near 4.4 Mbit/s | AP with one HT peer under a Block Ack agreement and one non-HT laptop; a 2 ms collection deadline applied to every destination (a defect) | 2026-09-26 | stand measurement (record outside this repository) | measured |
| AP TX to a non-HT laptop 19.2–19.7 Mbit/s | single ERP-OFDM MPDUs, six cycles | 2026-09-26 | stand measurement (record outside this repository) | measured |
| Legacy (non-HT) association completed the WPA2 handshake, then the station never reported Connected | OpenWrt AP in non-HT mode | 2026-10-04 | stand measurement (record outside this repository) | measured symptom; cause unconfirmed |
| HE20: all 108 742 datagrams at ~80 Mbit/s, but publication/BlockAck counter reconciliation failed | early HE20 run | 2026-09-08 | stand measurement (record outside this repository) | measured, superseded |
| Observer perturbation ~3.7–4.1 % (100.3 vs 96.6 Mbit/s, same image) | older egress experiments, observer off vs on | 2026-09 (older snapshots) | stand measurement report (record outside this repository) | claimed: reported from snapshots, not rerun |
| Cached PSRAM (0x50xx_xxxx) addresses as Wi-Fi DMA buffers delivered no UDP; same image with SRAM delivered 122.35–122.40 Mbit/s | older egress experiments | 2026-09 (older snapshots) | same record | claimed; the DMA contract stays SRAM |
| One peer ~124 Mbit/s with early SRAM admission; two peers ~97 Mbit/s at 67 slots, ~121 at 98 slots (+52 576 bytes) | older AP queue-geometry experiments | 2026-09 (older snapshots) | same record | claimed |
| GDMA PSRAM→SRAM copy of 32 frames: fewer instructions but 2.27× wall time | older copy experiments | 2026-09 (older snapshots) | same record | claimed |
| Monitor mode received over 1000 frames | standalone monitor example, staged PSRAM boot, PSRAM at 250 MHz | 2026-09-05 | stand measurement (record outside this repository) | measured |
| Monitor capture scenario passed (with ICMP latency, station reconnect and AP ICMP) | Wi-Fi interrupt sources enabled through the image's interrupt table | 2026-10-04 | stand measurement (record outside this repository) | measured (pass, no numbers) |

### Power save and time

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| 353 RF sleeps of 41–304 ms; MAC local-time counter and monotonic time agree within 1 µs | station modem sleep (min-modem), DTIM 1 and 3 set on the AP | 2026-10-03 | stand measurement (record outside this repository) | measured: the MAC counter runs through RF sleep |
| 256 RF sleeps of 41–99 ms, agreement within 1 µs | max-modem, listen interval 10; max-modem woke for every beacon, so longer sleeps stay unmeasured | 2026-10-03 | stand measurement (record outside this repository) | measured |

## Bluetooth LE

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| DTM LE 1M 401 packets, LE 2M 400 → 798 packets received by the peer | ESP32-C5 vendor Controller counting; RF channel 19; LE 2M at 400 was half the vendor's rate because each second 625 µs slot was skipped (a 37-octet LE 2M packet leaves 433 µs to the next slot) | 2026-09-29 | stand measurement (record outside this repository) | measured |
| DTM RX on LE 1M, 2M, Coded S=8 and S=2 and TX on LE 1M and 2M against the C5 peer, at least 100 packets per case | RF channel 19, with an idle-transmitter silence control | 2026-09-28 | stand measurement (record outside this repository) | measured (pass); per-case counts not recorded in the sources |
| DTM to the Linux adapter: 80 packets delivered, zero false packets while silent, 200 Test End cycles and 100 Reset restarts | LE 1M; the run stopped at the adapter's own LE Test End timeout | 2026-09-27 | stand measurement (record outside this repository) | measured |
| High-duty-cycle directed advertising ended with Advertising Timeout after 1.28 s; scan response read by an active Linux scanner; low-duty directed advertising connected | Linux adapter | 2026-09-28 | stand measurement (record outside this repository) | measured |
| GATT 6 of 9, secure GATT 1 of 9 passed | Trouble GATT against BlueZ, before the cause was found | 2026-10-09 | stand measurement (record outside this repository) | measured |
| CONNECT_IND lost in 22 of 128 attempts (~17 %); foreign SCAN_REQ to ~21 % of ADV_IND (38 of 184 events); 4 of 33 lost with a nearby phone's Bluetooth off, 3 of the 4 with a foreign SCAN_REQ in the same event | advertising on channel 37 only; foreign active scanners with rotating random addresses; host and DUT traces aligned within 0.2 ms | 2026-10-09 | same record | measured: cause is the foreign scanner collision; whether the DUT also loses CONNECT_IND without a collision is open |
| 140 established connections, 0 lost by supervision timeout before the first ATT exchange (< ~2 % at 95 % confidence); not established 11 of 71, 1 of 31, 0 of 50 attempts | GATT 20 reps, traced GATT 10 reps, secure GATT 10 reps | 2026-10-11 | stand measurement (record outside this repository) | measured |
| One connection lost by supervision timeout (0x08) ~449 ms after creation, before any ATT exchange | interval 45 ms, supervision timeout 420 ms, the DUT had received the CONNECT_IND | 2026-10-09 | same record | measured, single event; cause open |
| 22 of 22 repetitions passed (GATT 3, traced GATT 10, secure GATT 9); the fixture retried 2 of 11, 9 of 39 and 6 of 51 attempts the DUT never received | several connections held by the Controller core | 2026-10-11 | stand measurement (record outside this repository) | measured |
| 18 of 20 passed (one secure repetition exhausted its three attempts, one GATT repetition lost the link by 0x08) | GATT and secure GATT, 10 + 10 | 2026-10-09 | stand measurement (record outside this repository) | measured; 10 of 10 never confirmed |
| Port poisoned in 1 of 79 secure GATT repetitions | joined BLE driver with lock-free request exchange; cause not reported by that build | 2026-10-12 | stand measurement (record outside this repository) | measured; cause unknown |
| Secure GATT 3 of 3; missing-key, key-refresh, wrong-key and ACL backpressure 3 of 3 each; missing-key termination reasons matched the vendor on the same board | Numeric Comparison pairing, encrypted ACL | 2026-09-17, 2026-09-20 | stand measurement (record outside this repository) | measured |

## IEEE 802.15.4

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| Energy -87 dBm baseline → -41/-42 dBm during the peer's burst, matching the frames' -42 dBm RSSI; -90 dBm on channel 25; -40 dBm after retuning back; -87/-86 dBm after the burst; 3 of 4 CCA-first transmissions found the channel busy | peer burst on channel 15, two boots | 2026-09-27 | stand measurement (record outside this repository) | measured |
| Live RSSI within 2 dB of the last frame's RSSI; followed a 36 dB step | peer transmit power 20 and -15 dBm | 2026-09-28 | stand measurement (record outside this repository) | measured |
| Acknowledged exchange, filtering, frame pending and enhanced ACK all passed | DUT at the PIB default 21 dBm, CSMA-CA with retries, two boots | 2026-09-28 | stand measurement (record outside this repository) | measured |
| ACK loss 7 of 20 repetitions, then 95 clean runs including 20 at the same image | peer sends 8 acknowledged frames ~2.5 ms apart; the DUT missed one frame for all three peer retries, without fault; failures clustered in one hour while the peer's USB console was wedging | 2026-10-03/04 | stand measurement (record outside this repository) | measured; no code regression established; layout/PSRAM handler hypothesis not supported |
| ED rejected as a coexistence reject with MODEM_LPCON.CLK_CONF = 0x00000001 (only the Wi-Fi power clock open, coexistence and analog-I2C gates closed) | ED probe whose helper dropped the platform clock references | 2026-10-11 | stand measurement (record outside this repository) | measured: closed coexistence clock causes the reject |
| Thread datagram exchange passed | C5 vendor OpenThread peer | 2026-10-11 | stand measurement (record outside this repository) | measured |

## Coexistence

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| Wi-Fi UDP RX with a BLE echo passed | joint Wi-Fi and BLE echo | 2026-09-28, 2026-10-04 | stand measurement (record outside this repository) | measured (pass, no numbers) |
| 802.15.4 peer exchange passed with the device taking part in Wi-Fi coexistence | | 2026-09-27 | stand measurement (record outside this repository) | measured (pass, no numbers) |
| Wi-Fi works after a full common-power cycle only when the RF wake and the coexistence enable both precede the repeated power entry; the reverse order failed 3 of 3 (station rejoin starves, watchdog reset) | `wifi-radio-restart`; skipping the baseband reset pulse or re-applying the entry's modem and PHY clock writes did not help (3 experiments) | 2026-10-11/12 | stand measurement (record outside this repository) | measured; which write conflicts is open (candidates: the platform clock references, the cold-baseline capture) |

## PHY

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| IEEE 802.15.4 idle tracking hold maxima: RFPLL 1.467 ms, common 8.350 ms, transmit 9.234 ms, baseband off 9.090 ms, whole tick 18.849 ms | s31-a, RX channel 15, forced `(3, 0)`, 3 repetitions × 5 due ticks; source `5cb90527`, timer-woken SAR deadline 40 µs; nested request intervals, not proof of RF exclusion | 2026-10-20 | HIL run `1792522303542-002f44c3`; [full table](../phy/README.md#measured-rf-hold-of-a-tracking-tick) | measured; 15/15 ticks valid, policies restored and RF closed |
| Extended 154 idle hold, layout seed 1: 20/20 repetitions × 5 ticks; maxima RFPLL 1.470 ms, common 8.921 ms, transmit 9.233 ms, baseband off 9.086 ms, whole tick 19.505 ms | source `5cb90527`; 21,470 SAR conversions, maximum ready observation 21 µs against the 40 µs deadline; no deadline failures | 2026-10-20 | HIL run `1792522693907-002ff8a0`; SOURCE: [timer readiness](https://github.com/ermacv/open-esp-radio/blob/5cb905278248c04fb9b597581df5bff4086cbf26/chips/esp32s31/phy/src/tx/sar.rs) | measured; 100/100 ticks valid, all policies restored and RF closed; observed maxima, no reliability bound |
| Wi-Fi cold calibration: MATCH, 19/19 reviewed field groups (96 elements), 162/162 MMIO and 35/35 analog. Vendor/production ranges: `WORD_0060_OPAQUE` [4,30888]/[0,0]; 61.09/61.0a [2,26]/[2,15]; 62.01/62.05 [13,14]/[13,13] | s31-a, 10 alternating single-boot pairs; source `b35b8a84` (main `2966eb81`, PHY `587104e6`, single-boot capture correction); unchanged vendor-spread gate | 2026-10-20 | HIL run `1792520182386-002ae184`, comparison.json | measured; discovery reboots are separate from measured boots; no field-specific PHY value changes |
| Temperature references match the actual vendor spread: common vendor [28,29] °C, production [29,29] °C; transmit/power/RFPLL vendor [25,26] °C, production [26,26] °C; margin 1 for each | same cold series; unreadable-MMIO discovery is separately archived, so a measured vendor capture no longer combines its initial object with later recovery boots | 2026-10-20 | same run, vendor-NNN.json and registers.log; SOURCE: `firmware/vendor/esp32s31/calibration/main/main.c`, `chips/esp32s31/knowledge/src/phy_relation/committed.rs` | measured; the recorded +2 °C does not establish a fixed sensor offset |
| Wi-Fi restart calibration: MATCH against 50 vendor reference boots, for both main and the timer-SAR implementation: 19/19 groups (96 elements), 163/163 MMIO, 38/38 analog | s31-a; ten alternating pairs on main code `a3ddd0a7` (`b55b11f0`, measurement-only changes; app `0e1279d0d3b2`), ten with SAR source `181d0831` (app `fb0f5c8d21c3`), plus thirty vendor-only restart boots; one pinned vendor application throughout; unchanged comparator/reviews | 2026-10-20 | HIL runs `1792524982715-00373aff`, `1792523107444-0030b722`, `1792525425321-0037a148`; original summaries reproduced before expanded-reference analysis | measured; each production series has ten complete captures and RF closures; initial ten-reference DIFF remains archived |
| Restart temperature references fit actual vendor spread: common vendor [27,30], main [30,30], timer-SAR [28,29] °C; TX/power/RFPLL vendor [25,28], main [26,27], timer-SAR [25,26] °C | same 50-reference series; vendor TX/power/RFPLL was constant 25 in the earlier ten boots, 26–28 in the control ten, and constant 26 in the extra thirty; between-series thermal variation contributes to the spread | 2026-10-20 | same runs; SOURCE: immutable registration projection and `knowledge/src/phy_relation/committed.rs` | measured; no fixed +2 °C correction is established |
| Restart SAR result registers fit the unchanged vendor-spread rule in both main and timer-SAR series: vendor `SAR_RESULT` [219417852,222039362], `SAR_RESULT_1` [219286802,221646134] | same 50-reference series; the original value-range DIFF also occurred with timer-woken readiness, while both capture sets complete | 2026-10-20 | same runs; SOURCE: `tx/dc_power_detector.rs` returns an error before consuming an unready sample; rev0 ROM `phy_pwdet_tone_start`, `phy_get_tone_sar_dout_`, `phy_get_fm_sar_dout` | measured; numeric sample spread is distinct from the reproduced hold readiness failure |
| `PHY_AGC_RECOVERED_GAPS.WORD_0050_OPAQUE` is a whole-word comparison artifact: known noise-floor low10 vendor [669,685], main [670,682], timer-SAR [678,682]; raw vendor word [765446822,3260040862] | same 50-reference series; upper22 semantics remain unknown; the unchanged whole-word widening now covers all u32 values and cannot establish calibrated-state equality | 2026-10-20 | same runs; SOURCE: complete rev0 ROM `phy_get_noise_floor` at 0x2f82571e reads 0x20107050 and ORs 0xfffffc00; `phy_get_rssi` consumes the resulting negative code | measured; the first ten-reference range was narrower in opaque upper bits; the low10 decode explains the data and does not replace the raw acceptance rule |
| Restart `COEX_LP_CLK_CONF` matches: vendor/main/timer-SAR [0,0]; historical 0x314/0 is a measurement/configuration artifact | same restart series; the pinned Wi-Fi reference does not enable software/external coexistence and contains no `init_coexist`; the exact earlier reference configuration is unavailable | 2026-10-20 | same runs; SOURCE: pinned IDF `4d59230d`, `components/esp_coex/src/coexist.c::init_coexist` guards `coex_pre_init`; complete libcoexist `coex_core_pre_init` / `coex_hw_timer_freq_set` select XTAL/50 when invoked | measured; no PHY regression reproduced |
| Wi-Fi after separating its cold reset from common power: cold MATCH 19/19 groups (96 elements), 162/162 MMIO, 35/35 analog; restart MATCH 19/19 (96), 163/163 MMIO, 38/38 analog | 10 alternating single-boot pairs per point, source `c6442ebf`, diagnostic app `f866a7260252`; Wi-Fi entry retains its post-calibration reset order | 2026-10-20 | HIL runs `1792529533499-003d8f91`, `1792529854173-003dbf4a` | measured; both commands exit 0, all captures complete and RF closed |
| IEEE 802.15.4 cold: MATCH 193/193 MMIO, 39/39 analog against 40 vendor boots; `I2C_TX_RATE_CONTROL = 0x01540002` in every vendor/production pair and after registration/common power | 5 production boots, source `c6442ebf`; raw N5 DIFF 190/193,37/39 is preserved; 5 current + 5 prior + 30 additional same-point vendor references, same pinned application; unchanged comparator | 2026-10-20 | HIL runs `1792530170243-003e5a9b`, `1792526108530-00387939`, `1792530494874-003f02ef`; SOURCE: complete ROM `phy_i2c_txrate_init`; source PHY performs that operation | measured; common power retains the MAC calibration window; original N5 summary reproduced before reference expansion; no cold/restart pooling |
| Cold SAR/BTAGC fit the unchanged vendor-width rule; vendor SAR0 [219286804,222563676], SAR1 [219286802,222825800], BTAGC WORD006C [251920565,251920580]; analog 62.01/.05 both vendor/production [2,3] | same 40-reference cold series; production SAR0 [220466484,223087986], SAR1 [219942172,222694726], BTAGC [251920559,251920578]; each reference range widened by its own width | 2026-10-20 | same immutable captures and comparison | measured; the 1-LSB analog exits fit actual vendor variation, without PHY value or comparison-rule changes |
| IEEE 802.15.4 restart: 193/193 MMIO, 38/39 analog; I2C TX rate matches `0x01540002` in all 5 pairs after initial and repeated entry; sole DIFF is 69.04, vendor [96,96], production [101,101] | source `4942344f` adds only a vendor-only scenario to `c6442ebf`; same runtime app; 5 alternating single-boot pairs; all RF closures complete | 2026-10-20 | HIL run `1792530906501-003f30d0`; pre-fix run `1792528143952-003b16f0` also has 96/101; SOURCE: typed `TEMPERATURE_SENSOR_SAR2_STATUS` low4, `Sar2Initialization` writes 5 | measured real warm initialization/status difference; linked-vendor SAR-I2C lifecycle differs (below); no new reset-fix regression or PHY correction is inferred |
| SAR2 close/wake observations: production69.04 remains `0x65` / init5 after registration, before close, after wake and before final close; vendor final `0x60` / init0, in both protocols | IEEE restart1 clean `35ceab5d`, Wi-Fi restart1 clean `388ab678`; byte-identical diagnostic app `8395332f3073`; existing warm references N6 IEEE193/193 MMIO,38/39 analog; N11 Wi-Fi MATCH163/163,38/38 under its unchanged69.04 exclusion | 2026-10-20 | HIL runs `1792532634016-00020f57`, `1792533179530-000408cc`; SOURCE: pinned IDF4d59230d linked `phy_override.c::phy_set_tsens_power` → `sar_tsens_ctrl.c` → `sar_periph_ctrl_common.c` / S31 `regi2c_ctrl_ll.h`; [linked-ELF facts](https://github.com/ermacv/open-esp-radio/pull/236) | real retained resource-lifecycle difference; vendor last-release powers SAR-I2C off, first-acquire pulses reset with1µs settle; OER temperature leaves only toggle TSENS. No direct analog-zero assignment in that vendor chain; attribution to reset uses SOURCE plus final captures, without an intermediate vendor sample. PHY/HAL/power/comparison unchanged; Wi-Fi MATCH does not imply SAR2 matches |
| IEEE 802.15.4 idle hold with the preserved I2C TX rate: RFPLL 1.457 ms, common 9.088 ms, transmit 8.789 ms, baseband off 8.643 ms, whole tick 18.670 ms | 3 repetitions × 5 ticks, source `c6442ebf`/runtime-identical `4942344f`; 3262 SAR conversions, maximum ready observation 21 µs, no expiry at 40 µs | 2026-10-20 | HIL run `1792531108337-003fdf50`; [table](../phy/README.md#measured-rf-hold-of-a-tracking-tick) | measured; 15/15 valid, policies restored/RF closed; nested request intervals, no individual-step bound or RF-exclusion proof |
| Bluetooth/802.15.4 TX roughly 12 dB above the requested power after RF wake without a gain republish | older diagnostic branch | 2026-10-06 (older branch) | stand measurement report (record outside this repository) | claimed: not reproduced; RSSI alone does not establish it |
| RFPLL fresh-sample maintenance skipped as expected; forced maintenance completed with zero correction; 65 Mbit/s HT40 RX without UDP loss | channel 13 HT40 | 2026-09-10 | stand measurement (record outside this repository) | measured |

## Interrupt latency and CPU load

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| FROM_CPU raise → handler: idle min 2.03, median 2.04, p99 2.08–2.09, max 2.14 µs; under 59.9 Mbit/s HT40 UDP RX min 2.05, median 2.08, p99 2.13–2.15, max 4.74 µs | core 0, Priority1 (the MAC interrupts' level), CPU 320 MHz, 3 × 1024 samples per load spread over the traffic window | 2026-10-12 | stand measurement (record outside this repository) | measured; an earlier "under traffic" row was taken before traffic started and was withdrawn |
| CPU load idle link: core 0 1.71–1.79 %, core 1 0.09–0.10 % | 100 kbit/s RX | 2026-10-12 | stand measurement (record outside this repository) | measured |
| CPU load at UDP RX ceiling 108.3–112.8 Mbit/s: core 0 54.2–55.3 %, core 1 32.6–33.9 %; on later builds 68.3–68.4 % / 47.5–47.6 % at unchanged throughput | HT40 | 2026-10-12 | same record | measured; the rise across builds was not bisected |
| CPU load core 0 / core 1: UDP TX ceiling 60.0–60.2 / 41.7–41.8 %; TCP RX 83.3 / 63.5 %; TCP TX 81.2 / 69.6 %; AP single-client RX 82.5 / 57.0 %; AP single-client TX 62.9 / 28.7 % | HT40 | 2026-10-12 | same record | measured |
| The CPU-load instrument changes no throughput: UDP RX +0.04 ± 0.40, UDP TX -0.13 ± 0.51 Mbit/s | 5 A/B pairs | 2026-10-12 | same record | measured |

## Power and brownout

| Value | Conditions | Date | Source | Status |
| --- | --- | --- | --- | --- |
| Brownout reset (reset code 0x0f) about 2 s after the 802.15.4 air check starts, on every run; the board re-enumerates ~0.2 s later | independent of cable, USB port (powered hub) and detector policy (bootloader mode 1, and ESP-IDF's application policy mode 0 level 7); peer exchange, maintenance, Thread and BLE scenarios pass on the same board | 2026-10-10/11 | stand measurement (record outside this repository) | measured: the supply droop is real; which step draws the current (maximum-power TX, ED/CCA, start-up tracking) is open |
| 802.15.4 peer exchange brownouts on the clean baseline | | 2026-09-28 | stand measurement (record outside this repository) | measured, earlier occurrence |
| Reset cause reads 1 (power-on) after a hub power cycle and 0x18 after a JTAG reset, on s31-a and c5-a | read through JTAG | 2026-10-09 | stand measurement (record outside this repository) | measured |

## Not hardware measurements

Two items carry no on-chip value: the beacon
receive-time and hardware beacon monitor registers (meaning not established)
and the second station group-key slot (the 25-entry key table is a
register-model fact; the receive-side lookup by key id is unproven).
