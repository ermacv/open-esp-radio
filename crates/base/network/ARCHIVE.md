# Network boundaries

This directory owns stack-facing contracts; chip radio execution belongs to
`crates/runtime`. No ESP32-S31 composition exists to link a network stack
since  (tag `legacy-radio-2026-10-16`).

| Path | Responsibility |
| --- | --- |
| `interface/` | Stack-neutral interface, link and error values |
| `../adapters/embassy-net/owned/` | Maintained Embassy/Xarxa packet-owner contract with explicit pools |

Selecting a patched stack does not create another physical radio owner or
another packet adapter.
