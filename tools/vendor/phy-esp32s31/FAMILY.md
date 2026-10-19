# oer-vendor-phy-esp32s31

The ESP32-S31 implementation of the [PHY cross-check's](../phy/README.md)
comparison port, with the chip's vendor calibration comparison
`oer-vendor-phy-esp32s31`
over the published radio-PHY partition and analog-I2C register images. The
family crate itself names no chip, and its captures are the chip-neutral
[`oer-vendor-phy`](../../hil/link/src/capture). No runner registers a
PHY scenario family.

```console
cargo test -p oer-vendor-phy-esp32s31
```
