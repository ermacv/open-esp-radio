# oer-protocols-network-wire

Allocation-free Ethernet II, VLAN tag, ARP, IPv4, IPv6, ICMPv4 echo, TCP and
UDP header fields, DHCP messages and the Internet checksum. Its one dependency is `oer-portable-wire`.

| Module | Fields |
| --- | --- |
| `ethernet` | Ethernet II header, `EtherType` values the network side names |
| `arp` | ARP for IPv4 over Ethernet |
| `checksum` | The Internet checksum and the IPv4 pseudo header |
| `vlan` | VLAN Tag Control Information and the enclosed EtherType |
| `ip` | IPv4 and IPv6 fixed headers, Differentiated Services field and DSCP |
| `transport` | TCP and UDP header bounds and ports |
| `icmp` | ICMPv4 Echo and Echo Reply |
| `dhcp` | DHCP's fixed fields, magic cookie and options, read and written |

A parse returns a checked value over the caller's bytes or a typed refusal;
a valid header of another kind (another EtherType, another IP protocol) is
not an error. Options and extension headers are not interpreted; a parse does
not check a checksum.
The EtherType values the IEEE 802.11 side itself needs (EAPOL, WAPI) belong
to `oer-protocols-ieee80211-wire`; the LLC/SNAP field it carries is a plain number.
