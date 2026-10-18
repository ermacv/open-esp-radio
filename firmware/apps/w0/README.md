# w0

The radio system's first run on the ESP32-S31 (W0.4): one Embassy task polls
the radio system's actor (`oer-esp32s31-radio-system`) and the Wi-Fi client
(`oer-esp32s31-ieee80211::startup`) together on the thread-mode executor of
software interrupt 0, with the SYSTIMER alarm as the time driver.

1. `join`: the actor registers the PHY domain (full calibration,
   `FullForCache`; the cache is not kept) and the backend powers, clocks and
   enters Wi-Fi.
2. `track`: the initial tracking, the Wi-Fi RX enable, channel 6 (20 MHz) and
   the MAC cold start.
3. `leave`: the client leaves and the actor closes RF.

The log prints the calibration identity, the calibration path and the
registration's duration, the longest not-ready runs of the TX DC and PWDET
SAR readiness waits against their bound, the RF calibration diagnostics, the
initial tracking's duration, the PHY temperature (sensor units) and the MAC
handshake samples, then `OER-RESULT: pass w0`.

```sh
cargo fw run w0 --board s31-a
```

Nothing is transmitted. eFuse is only read: the base MAC and its extension
word (`MAC_EXT`) for the calibration identity, and the station and
access-point addresses that esp-hal's `interface_mac_address` derives from
the base MAC. No RTC slow-clock calibration is supplied to the MAC
(`MacSlowClockCalibration::Unavailable`). The executor is the one of
software interrupt 0 because the time driver dispatches only there; the
radio executor's final line (P2) comes with W1.
