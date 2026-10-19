# oer-vendor-phy

The host half of the vendor-versus-production PHY calibration cross-check:
the comparison port (`comparison::Comparison`) a chip implements, and the
driver of the vendor IEEE 802.15.4 reference firmware (`reference`), which
configures the radio and reads the vendor side's register state over the
reference peer's line protocol. The crate names no chip; the captures it
compares are the chip-neutral
[`oer-vendor-phy`](../../hil/link/src/capture).

No HIL scenario family or workload runs the cross-check.

```console
cargo test -p oer-vendor-phy
```
