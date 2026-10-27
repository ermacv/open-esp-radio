# Shared IEEE 802.15.4 MAC values and transactions

`oer-espressif-ieee802154-mac-values` holds the parts of the typed IEEE 802.15.4 MAC surface
that the ESP32-S31 and ESP32-C5 share: configuration values and commands, the
event vocabulary with its enable and observation states, receive and transmit
abort reasons, state observations, the public LL enable sets, and the ordered
interrupt activation and teardown transactions. It contains no address and no
register access.

Each chip PAC keeps its generated register blocks and the raw register access
over them, because the chips differ in register geometry: the frequency-code,
coexistence-PTI and transmit-power field widths, the width of the event field,
the interrupt route (two-core source 132 on the ESP32-S31, one-core source 12
on the ESP32-C5), the modem ETM trigger words and the MAC-initialization
delays. The engine and HAL import the shared semantic vocabulary directly
from this crate. The C5 PAC also exposes it through its typed leases.

The C5 register leases stay in its PAC: they forward to the chip's raw
access with type conversions, so a mistake there is a type error rather than
a behavior, and sharing them would need a public generic API over chip
associated types. They move here only when a third chip with this MAC
appears or when lease ownership or ordering logic must change in both chips.

The engine's `Ieee802154LowLevel` in `oer-espressif-ieee802154-engine` remains the
driver boundary: each chip HAL implements it over its chip PAC. This crate sits
below the chip PACs and is not a driver interface. Its independently
implemented event observation retains unnamed lanes 7 and 13 separately,
without assigning them a meaning or granting acknowledgement authority.
Engine-specific receive status, diagnostic counters, ETM routes and ACK
timeout remain in the engine.
