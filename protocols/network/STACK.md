# Network boundaries

This directory owns stack-facing contracts; chip radio execution belongs to
[`exec`](../../chips/family/platform/executor/README.md). The HIL Wi-Fi station
image links the pinned Embassy/Xarxa network stack through
`portable/embassy/net`; the HIL `wifi-network` image links the network core in
its place. Direct normal stack dependencies belong to that
adapter and firmware, outside the protocol cores and runtimes.

| Path | Responsibility |
| --- | --- |
| `port/` | Stack-neutral owned `NetDevice`, Ethernet views, link epochs and typed refusals (`oer-protocols-network-port`) |
| `wire/` | Ethernet, VLAN, ARP, IP and transport header fields, the Internet checksum (`oer-protocols-network-wire`) |
| `core/` | A sans-IO core with one owner: ARP, IPv4 and UDP over one interface (`oer-protocols-network-core`). The HIL `wifi-network` image links it through `runtime/` |
| `runtime/` | The task that owns the core, its channel sockets and its local producer (`oer-protocols-network-runtime`) |
| `../../portable/embassy/net/` | Maintained Embassy/Xarxa packet-owner contract with explicit pools |

Selecting a patched stack does not create another physical radio owner or
another packet adapter.
