# Shared Wi-Fi MAC register transactions

`oer-ieee80211-pac` composes reviewed transactions over the Wi-Fi MAC register
blocks that several chips share; `raw/` is `oer-ieee80211-pac-raw`, the
generated blocks and transactions of the
[register library](../../../registers/ieee80211/README.md). Neither crate
contains an address.

Each function takes a register block by reference. A chip PAC passes its
addressed peripheral, `Periph<RegisterBlock, ADDRESS>`, which dereferences to
the block, so every call compiles to the same code as a chip-specific
implementation with the chip's constant address. Only the closed chip PACs and
HALs depend on these crates.

| Module | Transactions |
| --- | --- |
| `interface_address` | `hal_mac_set_addr`: publish one receive-interface address and enable its receive policy |

`MacInterface` is the reviewed interface selector shared by the address,
BSSID, crypto, TX and receive-BlockAck transactions.
