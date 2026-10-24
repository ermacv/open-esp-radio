# Shared register descriptions

Register layouts that Espressif chips share and each chip places at its own
base addresses. The [Wi-Fi MAC register library](ieee80211/README.md) is the
one such library; its output is the typed register blocks of
[`chips/espressif/ieee80211-pac`](../ieee80211-pac/README.md).

Each chip's own register model is in `chips/<chip>/registers/`: the
[ESP32-S31 model](../../esp32s31/registers/README.md) and the
[ESP32-C5 model](../../esp32c5/registers/README.md). A model separates
editable hardware semantics, publication policy, upstream inputs and
generated outputs; its PAC is `chips/<chip>/pac`. The ESP32-C5 model
publishes SVD and binding indexes; its PAC is `chips/esp32c5/pac`.

Loading, validation and generation are the
[register tool](../../../tools/registers/generate/README.md)
(`cargo registers`). The vendor comparison reads the same models with its
own artifacts and comparison policy.
