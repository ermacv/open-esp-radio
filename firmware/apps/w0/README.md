# w0

The radio system's first run on the ESP32-S31. The radio system's
actor (`oer-esp32s31-radio-system`) runs on the radio executor (P2), polled
in the handler of `FROM_CPU` line 2; the Wi-Fi client
(`oer-esp32s31-ieee80211-driver::startup`) runs on the thread-mode application
executor of software interrupt 0. Each executor has its SYSTIMER comparator
(the radio executor's alarm latches at P3); the radio executor writes the
system timer, and `LEVELS` checks the level order when the image compiles.

1. `join`: the actor registers the PHY domain (full calibration,
   `FullForCache`; the cache is not kept) and the backend powers, clocks and
   enters Wi-Fi.
2. `track`: the initial tracking, the Wi-Fi RX enable, channel 6 (20 MHz) and
   cold MAC preparation: handshake, configuration and IRQ policy, with every
   common MAC gate held closed. W0 checks those gates before release; it
   publishes no BASE and does not call runtime MAC start without a ring.
3. `leave`: the client leaves and the actor closes RF.

The log prints the calibration identity, the calibration path and the
registration's duration, the longest not-ready runs of the TX DC and PWDET
SAR readiness waits against their bound, the RF calibration diagnostics, the
initial tracking's duration, the PHY temperature (sensor units) and the MAC
handshake samples with `mac_prepared=true`, then `OER-RESULT: pass w0`.
Runtime MAC up is exercised by W1/W2/R1 with an RX-ring proof.

```sh
cargo fw run w0 --board s31-a
```

Nothing is transmitted. eFuse is only read: the base MAC and its extension
word (`MAC_EXT`) for the calibration identity, and the station and
access-point addresses derived from the base MAC by the HAL's
`platform::FactoryIdentity::address`. No RTC slow-clock calibration is
supplied to the MAC (`MacSlowClockCalibration::Unavailable`).
