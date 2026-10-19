# ESP32-S31 coexistence binding

This crate binds the Espressif coexistence policy of
[`oer-espressif-coex`](../../../../portable/espressif/coex/README.md) to the ESP32-S31
radio domain and re-exports that policy. The recovered timer programming
sequence, clock conversion, event-to-timer mapping, priority table and
time-slice schedule live in the family crate; this crate keeps what reads or
writes S31 registers:

- `CoexDomainPorts` lends one radio domain's timer bank, event priorities
  and clock to `CoexCore` as its `CoexTimerHardware` and `CoexClockHardware`
  ports. The domain lends only timers 0 through 4: timer 5 carries its PHY
  grant-protect request, so event 48 has no policy timer.
- The clock port decodes the HAL's coexistence low-power clock observation
  into the core's `CoexTimerClock`, with the 40 MHz crystal of every
  supported board profile.

No runtime composes the core and the schedule on the radio domain; the
policy a radio runtime runs is recorded in the
[Espressif coexistence policy](../../../../portable/espressif/coex/README.md#participants-status-and-order).
This crate does not implement
RF grant notification. See [source capabilities](FEATURES.md) for those
boundaries.
