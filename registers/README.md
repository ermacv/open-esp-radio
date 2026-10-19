# Reviewed register descriptions and publication

This tree owns the hardware descriptions and reviewed policies shared by
production PAC publication and vendor analysis. It contains source data,
provenance and checked generated catalogs; it is not a runtime crate or a
vendor investigation.

The [ESP32-S31 model](esp32s31/README.md) distinguishes editable hardware
semantics, publication policy, upstream inputs and generated outputs. Runtime
register owners and both generated Rust outputs remain in
[`chips/esp32s31/pac`](../chips/esp32s31/pac/README.md).
The [ESP32-C5 model](esp32c5/README.md) has the same layout; it publishes
SVD and binding indexes only, since no ESP32-C5 PAC crate exists yet.
The [Wi-Fi MAC register library](ieee80211/README.md) owns layouts a chip
places at its own base addresses; its outputs are
[`chips/espressif/ieee80211-pac`](../chips/espressif/ieee80211-pac/README.md).

Generic loading, validation and generation remain in
[register tool](../tools/registers/generate/README.md). A vendor project references the
same reviewed model while selecting its own artifact context and comparison
policy. Neither source generation nor binary analysis decides product readiness.
