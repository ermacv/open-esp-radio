# Wi-Fi RX: the local-time stamp in the RX prefix

Source: W1 hardware runs on ESP32-S31 rev0 (board s31-a), receive-only on
channel 6, 2026-10-18 (`firmware/apps/w1`, block lines `w1.bss`, `w1.lag`,
`w1.lagfit`). SOURCE[HIL_W1_RX_STAMP_2026_10_18]. Facts only.

The RX prefix of each received unit carries a 32-bit MAC local time at
prefix offset 0x0c (`decode_rx_local_timestamp`,
`crates/esp32s31/ieee80211/mac/src/rx/prefix.rs`).

- **Unit: microseconds, on the SYSTIMER rate.** Over a 10 s window the span
  of the stamps of one BSS equals the span of their P2 handoff times
  (SYSTIMER µs) to within 0.1 %; for the strongest AP 9 830 492 µs against
  9 830 491 µs. The offset between the two clocks is arbitrary.
- **Not unique per frame.** Of 64 consecutive units, 20 shared their stamp
  with the next unit, while the handoffs of the two differed by 1–10 ms or
  more. The stamp therefore marks something coarser than the arrival of
  each frame (which event is not known yet).
- **Consequence:** a per-frame delay computed as handoff − stamp is not a
  reception latency; only its trend over a window is meaningful. Per-frame
  timing must come from another source, for example a beacon's own TSF.
  This matters at W3 for TSF synchronisation and power save.
