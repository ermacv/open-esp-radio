# Network boundaries

This directory owns stack-facing contracts; chip radio execution belongs to
[`exec`](../../chips/espressif/executor/README.md). W3 and the HIL Wi-Fi station
image link the pinned Embassy/Xarxa network stack through
`portable/embassy/net`. Direct normal stack dependencies belong to that
adapter and firmware, outside the protocol cores and runtimes.

| Path | Responsibility |
| --- | --- |
| `src/` | Stack-neutral interface, link and error values (`oer-network`) |
| `../../portable/embassy/net/` | Maintained Embassy/Xarxa packet-owner contract with explicit pools |

Selecting a patched stack does not create another physical radio owner or
another packet adapter.
