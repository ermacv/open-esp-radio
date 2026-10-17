# ESP32-S31 coexistence binding

This crate binds the Espressif coexistence policy of
[`oer-espressif-coex`](../../../espressif/coex/README.md) to the ESP32-S31
radio arbiter and re-exports that policy. The recovered timer programming
sequence, clock conversion, event-to-timer mapping, priority table and
time-slice schedule live in the family crate; this crate keeps what reads or
writes S31 registers:

- `CoexArbiterPorts` lends one arbiter lease's timer bank, event priorities
  and clock to `CoexCore` as its `CoexTimerHardware` and `CoexClockHardware`
  ports. The arbiter lends only timers 0 through 4: timer 5 carries its PHY
  grant-protect request, so event 48 has no policy timer.
- The clock port decodes the HAL's coexistence low-power clock observation
  into the core's `CoexTimerClock`, with the 40 MHz crystal of every
  supported board profile.
- `validation` (feature `validation-probes`) runs the production core and
  timer sequence against an isolated validation radio for the compiled
  vendor comparison.

No runtime composes the core and the schedule under the arbiter lease now;
the policy the former radio runtime ran is recorded in the
[Espressif coexistence policy](../../../espressif/coex/README.md#participants-status-and-order).
This crate does not implement
RF grant notification. See [source capabilities](FEATURES.md) for those
boundaries.
