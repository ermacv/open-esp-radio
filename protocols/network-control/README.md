# Network HIL vocabulary

`oer-network-control` owns the bounded network session, traffic, readiness and evidence codecs used by images and host families. It is a sibling of `protocols/network`, which is a Cargo package and cannot contain another package. These are transport-session observation and experiment messages, including their existing Wi-Fi interface vocabulary; this crate does not define a product IP-service API.

The `registry` feature exposes descriptors and the reviewed `messages.lock`. The host composes them with the other dictionaries. CRC-32C evidence digests and all message schemas retain their wire representation.
