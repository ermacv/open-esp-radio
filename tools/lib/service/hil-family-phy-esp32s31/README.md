# oer-tool-service-hil-family-phy-esp32s31

The ESP32-S31 implementation of the [PHY cross-check's](../phy/README.md)
comparison port, with the chip's vendor calibration comparison
[`oer-tool-service-phy-vendor-calibration-esp32s31`](../../../phy/esp32s31/calibration/README.md)
over the published radio-PHY partition and analog-I2C register images. The
family crate itself names no chip, and its captures are the chip-neutral
[`oer-tool-service-phy-calibration-capture`](../../../phy/capture). No runner registers a
PHY scenario family ().

```console
cargo test -p oer-tool-service-hil-family-phy-esp32s31
```
