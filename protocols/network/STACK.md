# Network boundaries

This directory owns stack-facing contracts; chip radio execution belongs to
[`exec`](../../chips/espressif/executor/README.md). The HIL Wi-Fi station
image links the pinned Embassy/Xarxa network stack through
`portable/embassy/net`. Direct normal stack dependencies belong to that
adapter and firmware, outside the protocol cores and runtimes.

| Path | Responsibility |
| --- | --- |
| `port/` | Stack-neutral owned `NetDevice`, Ethernet views, link epochs and typed refusals (`oer-network-port`) |
| `wire/` | Ethernet, VLAN, IP and transport header fields (`oer-network-wire`) |
| `../../portable/embassy/net/` | Maintained Embassy/Xarxa packet-owner contract with explicit pools |

Selecting a patched stack does not create another physical radio owner or
another packet adapter.
