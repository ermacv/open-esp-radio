# Bluetooth LE wire formats

`oer-bluetooth-le-wire` owns stateless LE addresses, channel indices,
advertising and data PDU envelopes, CONNECT_IND fields, LL Control bodies,
data-length fields and standard Direct Test Mode payload generation.
It has no clock domain, protocol state or backend dependency.

`LeDeviceAddress` stores air-interface octets. `LePublicDeviceAddress` is an
explicit public backend identity with canonical and air-order accessors.
Composition translates its identity bytes at the HCI/backend boundary.

`advertising_header::AdvertisingHeader` is the single bitfield definition of
the advertising physical-channel header, including named primary-channel PDU
types. Advertising encoders and the advertising, scanning and CONNECT_IND
decoders use it. `LegacyAdvertisingPduPrefix` decodes the header and leading
AdvA without admitting the complete payload; backend profile selection precedes
packet extent admission. The full advertisement codecs validate their complete
payloads. The scanner retains its six-bit legacy length view; CONNECT_IND also
checks that view's reserved bits.
