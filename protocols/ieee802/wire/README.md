# IEEE 802 wire

`oer-protocols-ieee802-wire`: what every IEEE 802 wire carries alike. An `Eui48` is an
address field's octets, whatever they are; `IndividualAddress` and
`GroupAddress` are the kinds its I/G bit tells apart, kept by whoever admits a
frame. The IEEE 802.11 and the network codecs stand on it; its one dependency
is `oer-portable-wire` (`oer-tools-test-layers`).
