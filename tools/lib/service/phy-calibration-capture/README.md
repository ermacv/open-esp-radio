# oer-phy-calibration-capture

The chip-neutral capture of the vendor-versus-production PHY calibration
cross-check: what the HIL `phy` family
([`oer-hil-family-phy`](../../host/family/phy/README.md)) records on the
board under test and a chip's comparison reads.

- `boots`: the lifecycle point, each vendor and production boot and the
  images both sides ran, as the run bundle's typed observations;
- `space`: the register spaces both sides read (memory-mapped words and
  analog-I2C bytes) and a register of an image;
- `vendor`: the vendor calibration firmware's console protocol.

What a chip compares, and which register images it reads, belong to that
chip's comparison, for the ESP32-S31
[`oer-esp32s31-phy-vendor-calibration`](../esp32s31/calibration/README.md).
It is a package of the root workspace, which the HIL family links.

```console
cargo test -p oer-phy-calibration-capture
```
