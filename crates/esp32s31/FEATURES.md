# ESP32-S31 radio subsystem source capabilities

This page is the stable entry point for shared radio ownership, cold power and
clock setup, protocol handoff, powered-idle transitions, shutdown and
cross-protocol arbitration. The protocol and PHY inventories remain
independently owned and are linked below rather than duplicated here.

Exact bounded facts do not promote broader multi-client ownership, protocol
lifetime or concurrent arbitration claims.

## Coverage index

- [Shared PHY](phy/FEATURES.md)
- [Coexistence](radio/coex/FEATURES.md)
- [Wi-Fi](ieee80211/driver/FEATURES.md)
- Bluetooth and IEEE 802.15.4 have no capability page in this tree

## Exclusive ownership and client handoff

Exclusive root ownership and inactive route reselection do not establish safe
concurrent sharing or an active protocol switch.

## Active operation, power saving and shutdown

Protocol power-save policy, PHY maintenance admission and physical RF/clock
shutdown remain distinct boundaries.

## Concurrent ownership and arbitration

Standalone protocol paths do not combine into a joint-radio runtime.
