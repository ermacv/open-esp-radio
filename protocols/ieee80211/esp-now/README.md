# ESP-NOW protocol owners

`oer-ieee80211-esp-now` contains the sans-IO ESP-NOW peer, security,
duplicate-detection and bounded v2 reassembly owners, and the Espressif
long-range PHY requests of an ESP-NOW peer. It forbids unsafe code and
neither waits nor performs I/O. Frame codecs live in `oer-ieee80211-wire`;
transmit sequences, CCMP packet numbers and receive replay frontiers belong
to `oer-ieee80211-upper-mac`.

Wi-Fi VIF identities, channel-context bindings and shared topology validation
belong to `oer-ieee80211-arbiter`. ESP-NOW protocols take its
`interface::BoundVirtualInterface` to identify their station VIF.
`configuration::WifiStandaloneEspNowPlan::new` narrows an arbiter `WifiPlan`
to a station-address VIF without an AP or monitor sharing the radio. It
returns `None` for other checked topologies and grants no association,
authentication or network-stack authority.

`extensions/espressif/esp_now/protocol.rs` owns plaintext v1/v2 peers and
protocol state. Its `security.rs` sibling owns zeroizing secrets, encrypted
peer generations and anti-replay state; its peer table reports peer
changes, transmit packet-number allocations and replay admission as
`EspNowSecurityEvent`s to the sink each call lends, and keeps no counters
and no sink. These owners do not claim to produce
or authenticate encrypted on-air bytes: the encrypted Action-frame CCMP
and backend callback contract remain unavailable. Caller-owned v2 datagram
storage lives in `reassembly.rs`; `extensions/espressif/phy.rs` contains peer
PHY requests. Radio installation, hardware keys, receive filters and DMA
remain outside these protocol owners.
