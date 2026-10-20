# oer-vendor-phy

The host half of the vendor-versus-production PHY calibration cross-check:
the comparison port (`comparison::Comparison`) a chip implements, and the
driver of the vendor IEEE 802.15.4 reference firmware (`reference`), which
configures the radio and reads the vendor side's register state over the
reference peer's line protocol. The crate names no chip; the captures it
compares are the chip-neutral
[`oer-vendor-phy`](../../hil/link/src/capture).

Its `scenario` and `workload` modules run the manually selected `[phy]`
calibration investigation. The S31 comparison crate composes the family into
`cargo hil`; every flash and restore uses the run's leased board image port.
`phy-vendor-calibration-cold` and `phy-vendor-calibration-restart` use ten
alternating boots per side. `phy-vendor-calibration-smoke` uses one pair with an RF restart and exercises
the tool without estimating the vendor's boot spread. DIFF and INCOMPLETE
remain failures, including in the smoke scenario; captures survive the verdict.
Vendor-only scenarios capture extra boots without a production comparison.

```console
cargo test -p oer-vendor-phy
```
