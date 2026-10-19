# Network boundaries

This directory owns stack-facing contracts; chip radio execution belongs to
[`exec`](../../../chips/exec/README.md). No ESP32-S31 firmware exists yet to link a
network stack.

| Path | Responsibility |
| --- | --- |
| `src/` | Stack-neutral interface, link and error values (`oer-base-network`) |
| `../../adapters/embassy/net/` | Maintained Embassy/Xarxa packet-owner contract with explicit pools |

Selecting a patched stack does not create another physical radio owner or
another packet adapter.
