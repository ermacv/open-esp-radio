# Bluetooth LE wire formats

`oer-bluetooth-le-wire` owns stateless LE addresses, channel indices,
advertising and data PDU envelopes, CONNECT_IND fields, LL Control bodies,
data-length fields and standard Direct Test Mode payload generation.
It has no clock domain, protocol state or backend dependency.

`LeDeviceAddress` stores air-interface octets. `LePublicDeviceAddress` is an
explicit public backend identity with canonical and air-order accessors.
Composition translates its identity bytes at the HCI/backend boundary.
