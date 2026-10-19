# ESP32-S31 Bluetooth source capabilities

Source coverage does not imply RF delivery or interoperability. Classic BR/EDR
remains source inventory.

See the [whole-radio feature map](../FEATURES.md) for shared ownership and
the [PHY concurrent clients](../../phy/README.md#concurrent-clients)
for cross-protocol composition limits.

## Legacy advertising and scanning

No Controller core or S31 radio role in this repository runs advertising,
scanning or peripheral connections.

## Capability advertisement

Each optional LE feature bit returned by the portable bootstrap requires a
complete production Controller operation.

## Peripheral timing limits

A peripheral composition must satisfy the Controller's clock accuracy and
captured-anchor requirements; a raw captured anchor is not a packet-start
timestamp.
