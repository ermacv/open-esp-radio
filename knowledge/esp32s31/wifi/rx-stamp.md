# Wi-Fi RX: the local-time stamp in the RX prefix

Source: W1 hardware runs on ESP32-S31 rev0 (board s31-a), receive-only,
2026-10-18 (`firmware/apps/w1`, block lines `w1.bss`, `w1.lag`, `w1.lagfit`,
`w1.tsffit`): on channel 6 among nine or more other BSSs, and on channel 13
with the stand AP alone. SOURCE[HIL_W1_RX_STAMP_2026_10_18]. Facts only.

The RX prefix of each received unit carries a 32-bit MAC local time at
prefix offset 0x0c (`decode_rx_local_timestamp`,
`chips/esp32s31/ieee80211/mac/src/rx/prefix.rs`).

- **Unit: microseconds, on the SYSTIMER rate.** Over a 10 s window the span
  of the stamps of one BSS equals the span of their P2 handoff times
  (SYSTIMER µs) to within 0.1 %; for the strongest AP on channel 6
  9 830 492 µs against 9 830 491 µs, for the stand AP on channel 13
  9 932 737 µs against 9 932 722 µs. The offset between the two clocks is
  arbitrary.
- **Not unique per frame on a busy channel.** On channel 6, of 64
  consecutive units 20 shared their stamp with the next unit, while the
  handoffs of the two differed by 1–10 ms or more. On channel 13, with the
  stand's beacons alone, handoff − stamp over 64 units varied by a median
  of 1 µs and at most 27 µs. Which event the stamp marks when it repeats is
  not known yet.
- **No delivery delay.** Measured against each beacon's own TSF (the AP's
  clock) instead of the stamp, the handoff of 64 stand beacons on channel 13
  deviates from a straight line by a median of 2 µs, p90 17 µs and at most
  28 µs, with a slope of −4 ppm. The millisecond "delays" computed from the
  stamp on channel 6 were therefore an artifact of the stamp, not of the
  receive path. This lifts the "known receive limitation" recorded in the W1
  image PR (#25).
- **Consequence:** a per-frame delay computed as handoff − stamp is a
  reception latency only where the stamp is unique; on a busy channel only
  its trend over a window is meaningful. Per-frame timing that must hold
  everywhere comes from another source, for example a beacon's own TSF; TSF
  synchronisation and power save depend on it.
