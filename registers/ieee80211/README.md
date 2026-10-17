# Wi-Fi MAC register library

This library owns Wi-Fi MAC peripheral layouts that are identical on several
chips apart from their base address. It has no chip, no address and no
reviews of its own:

| Path | Owner and purpose |
| --- | --- |
| `model/library.toml` | Library name, generated crate name and its layout fragments |
| `model/peripherals/` | Layout fragments: schema-2 peripherals without `baseAddress` or `[[review]]` |
| `policy/api.toml` | Reviewed transactions and domains over the shared blocks, citing every chip's evidence |
| `publication/registers.toml` | Library publication: evidence catalogs and output paths |

A chip uses a layout through a `[[shared-fragments]]` entry of its
`device.toml`, which names the library, the fragment, the chip's review file
(schema 1, `[[review]]` only, citing that chip's evidence) and the base address
of every peripheral of the fragment:

```toml
[[shared-fragments]]
library = "../../ieee80211/model/library.toml"
fragment = "peripherals/wifi-mac-interface-address.toml"
review = "peripherals/wifi-mac-interface-address.review.toml"
placement = { WIFI_MAC_INTERFACE_ADDRESS = 0x2010405C }
```

The chip's SVD publishes the placed layout like any other peripheral. Its raw
PAC re-exports the library's register module instead of generating a copy, so
`Periph<RegisterBlock, ADDRESS>` keeps the chip's address. A chip API may not
define transactions on a shared peripheral; they live here once.

```console
cargo registers validate --manifest registers/ieee80211/publication/registers.toml
cargo registers generate --check --manifest registers/ieee80211/publication/registers.toml
```

Regenerate every chip placing a changed layout as well. A layout that differs
on one chip (a register stride or a field position) is not shared: that chip
keeps its own fragment.
