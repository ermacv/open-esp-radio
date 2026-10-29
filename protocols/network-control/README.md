# Network HIL vocabulary

`oer-network-control` owns the bounded network session, traffic, readiness and evidence codecs used by images and host families. It is a sibling of `protocols/network`, which is a Cargo package and cannot contain another package. These are transport-session observation and experiment messages, including their existing Wi-Fi interface vocabulary; this crate does not define a product IP-service API.

The `registry` feature exposes descriptors and the reviewed `messages.lock`. The host composes them with the other dictionaries. CRC-32C evidence digests and all message schemas retain their wire representation.

`network/datagrams/*` describes the station image's UDP echo/sink/source
application. Its sequence/timestamp header is 16 bytes in big-endian order.
The trace-only tracker distinguishes provisional sequence gaps, reordering,
duplicates, malformed headers, packets older than its 64-position window and
saturation. It owns no socket or timer. Workload admission and socket lifetime
belong to the image; product networking keeps no workload counters.
