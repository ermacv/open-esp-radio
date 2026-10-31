# oer-network-wire

Allocation-free Ethernet II, VLAN tag, IPv4, IPv6, TCP and UDP header
fields. Its one dependency is `oer-wire`.

| Module | Fields |
| --- | --- |
| `ethernet` | Ethernet II header, `EtherType` values the network side names |
| `vlan` | VLAN Tag Control Information and the enclosed EtherType |
| `ip` | IPv4 and IPv6 fixed headers, Differentiated Services field and DSCP |
| `transport` | TCP and UDP header bounds and ports |

A parse returns a checked value over the caller's bytes or a typed refusal;
a valid header of another kind (another EtherType, another IP protocol) is
not an error. Options, extension headers and checksums are not interpreted.
The EtherType values the IEEE 802.11 side itself needs (EAPOL, WAPI) belong
to `oer-ieee80211-wire`; the LLC/SNAP field it carries is a plain number.
