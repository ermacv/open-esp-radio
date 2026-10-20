# ESP32-S31 PHY calibration cross-check

The S31 comparison implements `oer-vendor-phy::comparison::Comparison` and
composes the executable `[phy]` family into `cargo hil`. It compares pinned
vendor captures against the `diagnostic-phy` image through the reviewed
`oer-esp32s31-knowledge::phy_relation::committed` relation.

```console
cargo hil run phy-vendor-calibration-cold --chip esp32s31 --board s31-a
cargo hil run phy-vendor-calibration-restart --chip esp32s31 --board s31-a
cargo hil run phy-vendor-calibration-ieee802154 --chip esp32s31 --board s31-a
cargo hil run phy-vendor-calibration-ieee802154-restart --chip esp32s31 --board s31-a
cargo test -p oer-vendor-phy-esp32s31
```

Both Wi-Fi investigations alternate ten vendor and diagnostic boots on one
leased board. A separate, archived readability discovery precedes those pairs;
its potentially resetting reads supply no comparison values. Each measured
reference capture remains in one primary boot; a new reset makes it incomplete. `phy-vendor-calibration-smoke` uses one pair including an RF restart; it
exercises capture, not the vendor's real boot spread. DIFF and INCOMPLETE fail
in every scenario, with evidence retained. A vendor-only scenario captures
additional reference boots without claiming production equivalence.

The reference is the pinned `vendor-calibration` catalog image. It reports its
full-calibration object before Wi-Fi starts, then keeps RX active without an
association or modem sleep. It reports its actual home channel for the
diagnostic image. SOURCE: `firmware/vendor/esp32s31/calibration/main/main.c`.

The diagnostic image supplies no input cache and requires full registration
and `FullForCache`. Its immutable registration projection is captured before
Wi-Fi startup; indexed register reads happen with RX active at the cold point,
or after one verified RF close/wake and Wi-Fi rejoin at the restart point.
Capture uses public PHY snapshots/getters, without verification hooks.

`registers.rs` derives both images from the published model: the readable
RadioPhyPeripherals partition and the PhyI2cField analog image. Production reads
only vendor-readable MMIO indices, and each published analog byte once.
Resetting reads remain recorded as unreadable; missing vendor-readable
production data never counts as a match.

The IEEE 802.15.4 investigations alternate five boots per side using the pinned
`ieee802154-vendor-reference`: channel 15, 21 dBm, RX active, either after cold
bring-up or OFF/ON on the reference and verified RF close/wake/rejoin on the
diagnostic. The diagnostic disables automatic ACKs and exposes no TX operation.
These points compare registers/analog bytes only, with no calibration object.
`phy-vendor-calibration-ieee802154-smoke` uses one restart pair. Optional
`vendor_windows` and `vendor_transmit` retain extra reference-side investigation
observations without changing the comparison gate.

The diagnostic 802.15.4 path runs first-client tracking before RX and supports
RF close/wake/rejoin, capture and final RF closure. Failed operations retain
INCOMPLETE evidence; a one-pair smoke does not establish vendor spread.

`compare.rs` and `tolerances.toml` retain the reviewed rules: calibration
fields use the widest per-element vendor boot range as margin, each element's
own range widened by that margin; registers/analog bytes use their own vendor
range widened by their own spread. Unreviewed fields and missing readable
registers yield INCOMPLETE; values outside the reviewed spread yield DIFF.
The existing signedness, masks and lifecycle-specific exclusions apply.

Each run retains per-boot captures, console evidence, image digests, reference
ESP-IDF revision, readable/unreadable registers and the complete comparison
ranges. Calibration fields outside the committed relation remain uncovered.
These captures compare state; they do not qualify RF quality or timing.

`cargo hil run phy-tracking-hold-ieee802154-idle --chip esp32s31 --board s31-a`
uses three repetitions of five forced due ticks on the diagnostic image, with
154 RX active in idle. Observations retain boot/session-correlated raw intervals,
branch completion, per-repetition maxima (microseconds in Tracking, RFPLL,
common, transmit, baseband-off order), debug restoration and final RF closure.
Incomplete evidence fails. These are observed request intervals, not a measured
RF exclusion guarantee; nested intervals are not additive. No Wi-Fi hold workload
is exposed by this family.

The [recorded vendor reference](../../../chips/esp32s31/vendor/phy-baselines/README.md)
provides generated Wi-Fi cold bounds for labeled boards.
`cargo hil run phy-recorded-calibration-cold --chip esp32s31 --board s31-b`
captures only the diagnostic image and applies the same comparison rules;
no vendor image is built or flashed on this path.
