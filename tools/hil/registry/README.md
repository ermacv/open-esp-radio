# Host HIL registry

`oer-hil-registry` composes descriptors from the generic protocol and the Wi-Fi, network, Bluetooth, lower-MAC conformance, PHY probe and radio-health dictionaries. `DeviceImageKeys` assembles and validates Hello/key pages and renders known keys by their domain paths. Images derive advertised sets from mounted services; this host registry does not select them.

Each owner keeps its own `messages.lock`. `messages_lock()` merges those locks into the canonical wire contract used by run provenance. The regression tests check lock coverage, duplicate keys/paths, schema hashes, readable properties and the complete pre-extraction contract. The bisect command discovers owner locks in source trees and uses the same canonical merge, so moving a dictionary between packages does not change its wire identity.
