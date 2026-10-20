# ESP32-S31 Wi-Fi capability entry point

This tree has no S31 Wi-Fi station or access-point role, runtime or
integrating firmware; this page covers the lower MAC.

Architecture navigation: [whole-radio map](../../FEATURES.md),
[MAC TX](../mac/src/tx.rs), [MAC RX](../mac/src/rx.rs), and
[shared PHY](../../phy/FEATURES.md).

## Lower-MAC core

The driver core `LowerMacCore` lives in [`src/lower_mac.rs`](src/lower_mac.rs).
No ESP32-S31 backend implements the portable lower-MAC port yet; the port's
operation mapping and limits are in
[the port's README](../../../../protocols/ieee80211/port/README.md#implementers).
