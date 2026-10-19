# Shared notes

Recovered facts about vendor algorithms that Espressif chips share, kept as
data. Code cites them; nothing here is built.

| Path | Contents |
| --- | --- |
| `wifi/` | vendor Wi-Fi algorithms shared by the chips (`libpp`) |

Each chip keeps its own data beside its code:

| Path | Contents |
| --- | --- |
| `chips/<chip>/notes/` | research notes on vendor Wi-Fi, Bluetooth LE, IEEE 802.15.4 and PHY behaviour, platform notes, silicon observations, open questions |
| `chips/<chip>/vendor/artifacts.toml` | the pinned vendor libraries, ROMs and ESP-IDF files that facts are read from |
| `chips/<chip>/vendor/coverage.toml` | vendor code paths a comparison outside this repository never reached, with the decision and reason for each |
| `chips/esp32s31/vendor/accepted-losses.toml` | vendor roots a comparison may lose against an earlier evidence index, each with its reason |
| `chips/<chip>/vendor/facts/` | machine-written registries: function fingerprints and recovered names (never edited by hand) |
| `tools/hil/acceptance/<chip>/` | HIL scenarios with their numeric gates, as acceptance criteria |

A value from a source without a license is a fact here and nothing more; see
the [source policy](../../../docs/source-policy.md).
