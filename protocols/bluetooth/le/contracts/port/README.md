# Bluetooth LE port vocabulary

`oer-protocols-bluetooth-le-contracts-port` holds the vocabulary of the Bluetooth LE
radio's boundary and no boundary: no trait, right or lifecycle, and no
backend implements a Bluetooth LE port. No hardware, PHY membership, SRAM
addresses or HAL proofs appear in it.

- The clock domain: `LeRadio`, `LeInstant` and `LeWindow` on the LE radio's
  clock, which has no relation to monotonic time.
- The request values of every role: advertising sets and events, scanners
  and scan windows, peripheral connections and connection events, Direct
  Test Mode events, the filter accept list, the coexistence `Operation` of
  an event and regulatory-capped transmit power (`LeBand`, `LeTxCeiling`,
  `TxPower`).
- The capabilities a backend states (`LeCapabilities`, `LeRadioCapabilities`)
  and `Supported`, which checks a value against them when it is built; a
  timed value is placed with `RadioTiming::reservation`, the one reservation
  function, within the capabilities' horizon.
- `RadioTiming` and `ConnectionAllowances`: the timing a backend reports, from
  which the link layer computes how long each event may last.
- The outcomes: `EventResult`, `ReceivedPdu` and `TestReport`.
- `dtm::next_window`: the Direct Test Mode window arithmetic of Core Vol 6
  Part F. Transmitters keep the interval grid and skip unreachable slots;
  receivers keep their 5 ms windows.

Wire PDUs, channels and address bytes come from `oer-protocols-bluetooth-le-wire`.
