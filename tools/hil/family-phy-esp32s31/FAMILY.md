# oer-hil-family-phy-esp32s31

The ESP32-S31 implementation of the [PHY cross-check's](../family-phy/README.md)
comparison port, with the chip's vendor calibration comparison
`oer-hil-family-phy-esp32s31`
over the published radio-PHY partition and analog-I2C register images. The
family crate itself names no chip, and its captures are the chip-neutral
[`calibration_capture`](../family-phy/src/calibration_capture). The HIL runner registers the S31 composition of the generic PHY scenario
family; it selects the four Wi-Fi and IEEE 802.15.4 capture points on
`diagnostic-phy`. The diagnostic runs first-client maintenance before RX and
supports verified RF close/wake/rejoin and final RF closure.

```console
cargo test -p oer-hil-family-phy-esp32s31
```
