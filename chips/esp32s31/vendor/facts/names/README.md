# ESP32-S31 vendor function names

These maps give the source names of generated function names in the pinned
ESP32-S31 Controller archive
[`espressif/esp32s31-bt-lib@7a85006f35f6839ae7ea5adee16e68a757759196`](https://github.com/espressif/esp32s31-bt-lib/tree/7a85006f35f6839ae7ea5adee16e68a757759196).
`cargo vendor-check lineage` carries names from
the initial archive commit `31c3094`, which has source names, through every
later commit that changes each library.

| Map | Library | Recovered | Without a name |
| --- | --- | --- | --- |
| [`libble_app.toml`](libble_app.toml) | `libble_app.a` | 1610 | 1355 |
| [`libbtdm_common.toml`](libbtdm_common.toml) | `libbtdm_common.a` | 220 | 26 |

`libbredr_app.a` has source names in every commit and needs no map. Each map
records the SHA-256 of every archive revision; the last one is the pinned
archive. A commit that leaves a library unchanged does not appear; the last listed
revision supplies the pinned archive bytes.

Names were produced with `--obfuscated-prefix r_sym_ --obfuscated-prefix sym_
--obfuscated-prefix brk_sym_` and the default policy: later archives add the
`sym_` and `brk_sym_` generated-name schemes. A function without a name first
appears after `31c3094` or has an unbroken chain nowhere. Each entry's
`weakest` field is the weakest evidence of its chain:

| Evidence | `libble_app.toml` | `libbtdm_common.toml` |
| --- | --- | --- |
| `exact-body` | 624 | 211 |
| `call-graph` | 354 | 5 |
| `similar` | 329 | 2 |
| `neighbourhood` | 146 | 2 |
| `dominant` | 157 | 0 |

`similarity-ppm` is the lowest body similarity along the chain. A name does
not describe current behavior: statements about a function still come from
its current body. Regenerate the maps with the command in the tool README
after a new archive revision, and record names of functions without an entry
by manual review in the reference that uses them.
