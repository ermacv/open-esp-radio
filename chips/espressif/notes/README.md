# Knowledge

Recovered hardware facts and research notes, kept as data. Code cites them;
nothing here is built.

| Path | Contents |
| --- | --- |
| `esp32s31/vendor/artifacts.toml` | the pinned vendor libraries and ROM that facts are read from |
| `esp32s31/vendor/coverage.toml` | vendor code paths the vendor comparisons of a stack outside this repository never reached: the decision for each, its reason and the port item that covers it |
| `esp32s31/vendor/facts/` | machine-written registries: function fingerprints and recovered names (never edited by hand) |
| `esp32s31/notes/` | research notes on vendor Bluetooth LE and IEEE 802.15.4 behaviour, and platform notes (bring-up order, core 1 sampling) |
| `esp32s31/bluetooth/` | facts from the unlicensed `esp32s31-bt-lib` |
| `esp32s31/wifi/` | rules the Wi-Fi driver keeps, and facts of another driver it decides anew |
| `esp32s31/open-questions.md` | open questions |
| `esp32s31/acceptance/` | HIL scenarios of a catalog outside this repository with their numeric gates, as acceptance criteria |
| `esp32c5/vendor/artifacts.toml` | the pinned ESP32-C5 vendor libraries, ROMs and ESP-IDF files |
| `esp32c5/vendor/coverage.toml` | ESP32-C5 vendor code paths a vendor comparison outside this repository never reached, in the ESP32-S31 format |
| `esp32c5/vendor/facts/` | machine-written registry of ESP32-C5 function fingerprints (never edited by hand) |
| `esp32c5/notes/` | ESP32-C5 pin relations, silicon observations and errata links |
| `esp32s31/phy/` | facts about the vendor PHY code from a vendor comparison outside this repository |
| `espressif/wifi/` | vendor Wi-Fi algorithms shared by Espressif chips (`libpp`) |

A value from a source without a license is a fact here and nothing more; see
the [source policy](../../../docs/source-policy.md).
