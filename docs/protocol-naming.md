# Radio protocol terminology

This reference defines technical names and namespace conventions. The
[architecture](architecture.md) defines concrete code owners.

## Protocol families

Use `ieee80211`, `ieee802154` and `bluetooth` for internal protocol families
at the portable, chip and adapter levels. A family namespace may contain an
explicit extension, local policy or silicon backend. It does not claim a
complete standard implementation or certification.

| Term | Meaning | Repository convention |
| --- | --- | --- |
| IEEE 802.11 | WLAN MAC and PHY specification family | `ieee80211` for internal protocol implementation |
| WLAN | Wireless local area network | A network/interface category, not an additional driver layer |
| Wi-Fi | Technology and ecosystem based on IEEE 802.11 | `wifi` is appropriate for user-facing APIs; it does not imply Wi-Fi certification |
| IEEE 802.15.4 | MAC/PHY family for low-rate wireless networks | `ieee802154`; each implementation states its PHY/channel/frame limits |
| WPAN | Wireless personal area network | A broader category, not a replacement name for the 802.15.4 backend |
| Bluetooth | Bluetooth SIG family, including BR/EDR and LE | `bluetooth`, with implemented LE procedures under `le` |
| HCI | Host–Controller commands, events and data transport | `bluetooth/hci`; neither an on-air protocol nor a complete Host stack |
| LE Link Layer | LE on-air PDUs and Link Layer procedures | `bluetooth/le/wire`, `bluetooth/le/ll` |

IEEE 802.15.4 does not itself provide Thread or Zigbee. HCI also applies to
BR/EDR and must not be named as if it were exclusively LE Link Layer.
`legacy advertising` in this code means legacy LE advertising.

The conventions follow the scope described by [IEEE 802.11](https://www.ieee802.org/11/abt80211.html),
[IEEE 802.15.4](https://standards.ieee.org/ieee/802.15.4/11041/),
[Wi-Fi technology terminology](https://standards.ieee.org/beyond-standards/the-evolution-of-wi-fi-technology-and-standards/)
and the [Bluetooth architecture](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-62/out/en/architecture%2C-change-history%2C-and-conventions/architecture.html).

## Responsibility matters more than symmetry

| Boundary | Owns | Does not imply |
| --- | --- | --- |
| Frame/wire/IE codecs | Packet representation, parsing and encoding | Radio or key ownership |
| Protocol/MAC/LL state | Windows, peers, retries and security transitions | MMIO, executor or hardware offload |
| Interface/contract | Values and capabilities exchanged by owners | A hidden peer/key store |
| Chip backend | Descriptors, timed execution, hardware MAC/LL and interrupt semantics | A complete upper stack |
| Adapter | Binding an existing capability to an external library, timer or CPU route | Another independent hardware owner |
| Integration | Resource selection, placement and final composition | Copied production algorithms |

Portable MAC mechanisms and a chip's hardware MAC backend may both use
`mac` within their different parents. Bluetooth retains `ll` for Link Layer.
Consistent ownership does not require identical protocol subdirectories,
shared frame types, IRQ queues or separate crates at every level.

No package is named for the division of MAC work between software and
hardware: the port's `MacOperationOwnership` is the per-operation contract.
The softmac and fullmac terms of
[Linux mac80211](https://wireless.docs.kernel.org/en/latest/en/developers/documentation/mac80211.html)
do not establish the same ownership here.

## Module and API names

Use path context instead of repeating a chip/protocol prefix on every private
type. For example, portable Bluetooth LE procedures live under
`bluetooth/le/ll/{scanning,connection,control}`; the chip IRQ and scheduler
remain in the hardware engine, outside those procedure modules. Each procedure
retains its own lifecycle owner.

Encoding a frame does not authorize installing a key or publishing DMA work.
WPA2 secrets and zeroization remain with their state owner; association
capability encoders stay beside the association encoder that uses them.
Chip AP profile selection is separate from portable advertisement encoding.

Public exports may retain chip/protocol prefixes to distinguish backends.
Reexports expose an API without creating another owner. Application code can
use the public `oer` library and context-specific local names such as
`RadioSystem` or `NetworkRunner`. Keep meaningful lifecycle names such as
`Prepared`, `Running` and `Quarantined`, including precise failure owners.

Vendor/upstream register names, SVD identifiers and Cargo package identities
retain their provenance. Namespace consistency does not require renaming
every package or creating folders for unimplemented protocol stacks.
