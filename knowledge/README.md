# Knowledge

Recovered hardware facts and research notes, kept as data. Code cites them;
nothing here is built.

| Path | Contents |
| --- | --- |
| `esp32s31/vendor/artifacts.toml` | the pinned vendor libraries and ROM that facts are read from |
| `esp32s31/vendor/coverage.toml` | vendor code paths the predecessor's comparisons never reached: the decision for each, its reason and the port item that covers it |
| `esp32s31/vendor/facts/` | machine-written registries: function fingerprints and recovered names (never edited by hand) |
| `esp32s31/notes/` | research notes on vendor Bluetooth LE and IEEE 802.15.4 behaviour, and platform notes (bring-up order, core 1 sampling) |
| `esp32s31/bluetooth/` | facts from the unlicensed `esp32s31-bt-lib`, in place of code that was not carried |
| `esp32s31/wifi/` | rules the Wi-Fi driver rebuild must keep, and predecessor facts it decides anew |
| `esp32s31/acceptance/` | the predecessor's HIL scenarios with their numeric gates, as acceptance criteria for future slices |
| `esp32c5/vendor/artifacts.toml` | the pinned ESP32-C5 vendor libraries, ROMs and ESP-IDF files |
| `esp32c5/vendor/coverage.toml` | ESP32-C5 vendor code paths the predecessor's comparison never reached, in the ESP32-S31 format |
| `esp32c5/vendor/facts/` | machine-written registry of ESP32-C5 function fingerprints (never edited by hand) |
| `esp32c5/notes/` | ESP32-C5 pin relations, silicon observations and errata links |

A value from a source without a license is a fact here and nothing more; see
the [source policy](../docs/source-policy.md).
