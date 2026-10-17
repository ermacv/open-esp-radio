# Knowledge

Recovered hardware facts and research notes, kept as data. Code cites them;
nothing here is built.

| Path | Contents |
| --- | --- |
| `esp32s31/vendor/artifacts.toml` | the pinned vendor libraries and ROM that facts are read from |
| `esp32s31/vendor/facts/` | machine-written registries: function fingerprints and recovered names (never edited by hand) |
| `esp32s31/notes/` | research notes on vendor Bluetooth LE and IEEE 802.15.4 behaviour |
| `esp32s31/bluetooth/` | facts from the unlicensed `esp32s31-bt-lib`, in place of code that was not carried |
| `esp32s31/wifi/` | rules the Wi-Fi driver rebuild must keep |

A value from a source without a license is a fact here and nothing more; see
the [source policy](../docs/source-policy.md).
