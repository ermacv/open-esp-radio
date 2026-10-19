# Network boundaries

This directory owns stack-facing contracts; chip radio execution belongs to
[`exec`](../../chips/espressif/executor/README.md). No ESP32-S31 firmware exists yet to link a
network stack.

| Path | Responsibility |
| --- | --- |
| `src/` | Stack-neutral interface, link and error values (`oer-network`) |
| `../../portable/embassy/net/` | Maintained Embassy/Xarxa packet-owner contract with explicit pools |

Selecting a patched stack does not create another physical radio owner or
another packet adapter.
