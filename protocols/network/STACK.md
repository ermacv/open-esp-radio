# Network boundaries

This directory owns the network and its contracts with the radio; chip radio
execution belongs to
[`exec`](../../chips/family/platform/executor/README.md). The network core is
the one network: the application entry starts it over a Wi-Fi role's port.

| Path | Responsibility |
| --- | --- |
| `port/` | Stack-neutral owned `NetDevice`, Ethernet views, link epochs and typed refusals (`oer-protocols-network-port`) |
| `wire/` | Ethernet, VLAN, ARP, IP, ICMP echo and transport header fields, DHCP messages, the Internet checksum (`oer-protocols-network-wire`) |
| `core/` | A sans-IO core with one owner: ARP, IPv4, UDP, a DHCPv4 client and ICMP echo over one interface (`oer-protocols-network-core`). The HIL `wifi-station` image links it through `runtime/` |
| `runtime/` | The task that owns the core, its channel sockets and its local producer (`oer-protocols-network-runtime`) |

A network instance does not create another physical radio owner.
