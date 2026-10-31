# oer-bridges-ieee80211-ethernet

The bridge between Ethernet II frames and IEEE 802.11 MSDUs. It depends on
the two codecs only: `oer-network-wire` and `oer-ieee80211-wire`.

`qos::classify_wmm` takes a frame's EtherType and the payload after its
header and returns the user priority the frame requests:

- An outer VLAN tag's Priority Code Point takes precedence over an
  encapsulated DSCP.
- An IPv4 or IPv6 DSCP maps through the station-safe RFC 8325 table: CS6,
  CS7 and unassigned codepoints fall to UP0.
- A valid frame of another type is `Unmarked`; a VLAN or IP header the codec
  refuses is `Malformed`. Both request best effort.

The mapping is a policy, not an authorization: a marking can request Voice.
The station decides the final priority from its link's QoS and admission
control before it numbers the frame.
