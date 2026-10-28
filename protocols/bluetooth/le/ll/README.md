# Bluetooth LE Link Layer

`oer-bluetooth-le-ll` owns allocation-free LE protocol state: connection and
channel progression, duplicate filtering, control, Data Length Update and
encryption procedures and packet counters. Stateless AES-CCM transforms and
zeroizing key material live in `oer-bluetooth-le-cipher`.
Air-format values and codecs are in `oer-bluetooth-le-wire`; DTM backend window
geometry is in `oer-bluetooth-le-port`.

HCI is the world boundary. This core exposes per-connection progression;
arbitration between conflicting connections and Host receive backpressure
are not implemented. The DTM runtime retains each HCI response until transport
delivery completes before releasing command credit. Backend faults cross the
port as typed failures; firmware owns shutdown and fail-stop policy.
