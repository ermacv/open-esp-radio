# Shared Wi-Fi MAC register blocks

`oer-espressif-ieee80211-pac` holds the typed register blocks of the Wi-Fi MAC
layouts that several chips share, generated from the
[register library](../registers/ieee80211/README.md) into `src/generated.rs`
(no `unsafe`, no address), and the transactions composed over them. A block's
owner is generic over its placement: a chip PAC's generated module names the
chip's address, claim and owner at that address, and the chip's radio root
claims it, so every call compiles to the same code as a chip-specific block
at the chip's constant address. Tests place a block in host memory. Only the
closed chip PACs and HALs depend on this crate.

| Module | Transactions |
| --- | --- |
| `interface_address` | `hal_mac_set_addr`: publish one receive-interface address and enable its receive policy |

`MacInterface` is the reviewed interface selector shared by the address,
BSSID, crypto, TX and receive-BlockAck transactions; `MacInterface::index`
selects its element of the four-entry interface register arrays.
