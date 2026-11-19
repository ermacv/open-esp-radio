# Bluetooth LE Link Layer

`oer-protocols-bluetooth-le-ll` owns allocation-free LE protocol state: connection and
channel progression, duplicate filtering, control, Data Length Update and
encryption procedures and packet counters. `timing` computes connection-event
and legacy advertising geometry from the timing allowances a backend reports
in the vocabulary of `oer-protocols-bluetooth-le-contracts-port`; `receive` authenticates received data channel
PDUs before Link Layer and Host dispatch. Stateless AES-CCM transforms and
zeroizing key material live in `oer-protocols-bluetooth-le-cipher`.
Air-format values, codecs and LE 1M air-time constants are in
`oer-protocols-bluetooth-le-wire`; the DTM window arithmetic is in
`oer-protocols-bluetooth-le-contracts-port`.

HCI is the world boundary. This core exposes per-connection progression;
arbitration between conflicting connections and Host receive backpressure
are not implemented.
