# oer-vendor-phy

The host half of the vendor-versus-production PHY calibration cross-check:
the comparison port (`comparison::Comparison`) a chip implements, and the
driver of the vendor IEEE 802.15.4 reference firmware (`reference`), which
configures the radio and reads the vendor side's register state over the
reference peer's line protocol. The crate names no chip; the captures it
compares are the chip-neutral
[`oer-vendor-phy-calibration-capture`](../../../phy/capture).

No HIL scenario family or workload runs the cross-check: its scenarios and the
production images they booted were deleted with the old radio stacks (),
and return with the HIL architecture of . The tag
`legacy-wifi-2026-10-16` keeps the former `[phy]` family and its
`vendor-calibration` workload.

```console
cargo test -p oer-vendor-phy
```
