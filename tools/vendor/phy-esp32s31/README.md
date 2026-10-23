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
Wi-Fi startup; indexed register reads sample the selected cold point, or the state after one
verified RF close/wake and Wi-Fi rejoin at the restart point. The diagnostic
RX experiment runs after the host has read all capture pages.
Capture uses public PHY snapshots/getters, without verification hooks.

`registers.rs` derives both images from the published model: the readable
RadioPhyPeripherals partition and the native analog-I2C capture image. Production reads
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
The typed `initial-pair = "rx-dc-gain0"` review also accepts a later
shared or Wi-Fi RX-DC gain pair when both codes equal gain0 of that same
capture. Gain0 and Wi-Fi fine-calibration pairs retain the range rule.
Each acceptance outside the vendor bounds appears under the field's
`initial-pairs`, with capture index, gain, pair and the reason
`initial pair, vendor-faithful`. Summary schema 6 retains these rows;
tolerance schema 4 defines the reviewed rules. The live and recorded comparisons
use the same per-capture check, including when paired vendor boots are
projected as the CONTROL subject. Reported bounds and margins remain measured.
`excluded` skips only value comparison. Every vendor-readable register,
including an excluded one, must be read in every production boot. A missing
or unreadable production sample yields INCOMPLETE.

The typed `reference = "same-run-control"` review for XTAL duty bytes
`analog-i2c.61.09` and `analog-i2c.61.0a` requires the vendor CONTROL from the
same complete alternating run. It uses the existing spread formula and the
unchanged masks; every byte must still be read. The vendor has shifted between
44–47, 2–17 and 43–58 on one board at overlapping 23–25 °C temperatures; the
physical cause is unknown. Other recorded bounds remain fixed. Recorded-only
comparison without CONTROL leaves these bytes explicitly INCOMPLETE and never
builds or flashes a vendor image to supply the missing control.
The register summary's `same-run-control` rows retain the rule reason, CONTROL
capture count, measured vendor/production bounds, margin and verdict, including
INCOMPLETE when CONTROL or a mandatory read is missing. A Wi-Fi CONTROL requires
at least ten paired boots (five for IEEE 802.15.4); a smoke pair is insufficient.

The recipe fingerprint hashes production capture/comparison sources, including
the full-registration/cache exchange. Test bodies live in external modules and
are excluded. A production recipe change leaves an older recorded reference
INCOMPLETE until new captures are obtained; moving tests does not authorize
re-signing captures from a different production recipe.

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
RF exclusion guarantee; nested intervals are not additive.
`phy-tracking-hold-wifi-idle` runs the same three-by-five idle investigation with
Wi-Fi active. Before each tick it makes the retained I2C band stale, so ordinary
T3 tracking executes and commits both masked writes. Family 3 UART observations
retain its P1 poll counts, maxima and timer lateness separately from T1 and T7.
Associated Wi-Fi traffic is a separate workload.

The [recorded vendor reference](../../../chips/esp32s31/vendor/phy-baselines/README.md)
provides generated Wi-Fi cold bounds for labeled boards.
`cargo hil run phy-recorded-calibration-cold --chip esp32s31 --board s31-b`
captures only the diagnostic image and applies the same comparison rules;
no vendor image is built or flashed on this path.
