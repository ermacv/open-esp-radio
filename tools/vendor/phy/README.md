# oer-vendor-phy

The host half of the vendor-versus-production PHY calibration cross-check:
the comparison port (`comparison::Comparison`) a chip implements, and the
driver of the vendor IEEE 802.15.4 reference firmware (`reference`), which
configures the radio and reads the vendor side's register state over the
reference peer's line protocol. The crate names no chip; the captures it
compares are the chip-neutral
[`oer-vendor-phy`](../../hil/link/src/capture).

Its `scenario` and `workload` modules run the manually selected `[phy]`
calibration and tracking-hold investigations. The S31 comparison crate composes the family into
`cargo hil`; every flash and restore uses the run's leased board image port.
`phy-vendor-calibration-cold` and `phy-vendor-calibration-restart` use ten
alternating boots per side. `phy-vendor-calibration-smoke` uses one pair with an RF restart and exercises
the tool without estimating the vendor's boot spread. DIFF and INCOMPLETE
remain failures, including in the smoke scenario; captures survive the verdict.
The IEEE 802.15.4 cold/restart investigations use five pairs, and
`phy-vendor-calibration-ieee802154-smoke` uses one restart pair. They drive the
live reference peer and compare register/analog captures without calibration
objects. Optional vendor windows and post-transmit captures are archived as
investigation data. Vendor-only scenarios capture extra boots without a
production comparison.

```console
cargo test -p oer-vendor-phy
```

The `tracking-hold` and `tracking-hold-wifi` kinds request 1–16 due ticks per
batch of an active diagnostic 154 or Wi-Fi RX session. `policy = "forced"`
(the default) selects debug `(3, 0)` and forces the retained power/Wi-Fi I2C
state before every tick; `policy = "natural"` leaves that state and the debug
policy unchanged. `batches` is 1–20 (default 1) under one RF registration.
The two `phy-tracking-hold-*-idle` scenarios use three boots of five forced
ticks. `phy-tracking-hold-wifi-ordinary` uses 20 batches of 16 natural ticks.

The same observer records coarse raw intervals, actual committed branches,
root polls/suspensions and engine counters in both modes. All UART reports
follow the root future's return. Each batch requires completion and debug
policy restoration/checking; final cleanup requires RF closure even on error.
Invalid pages or failed cleanup remain INCOMPLETE. Associated traffic and
continuous RF exclusion are separate measurements.

Reference readability is discovered in a separate capture before the alternating
series. Discovery retains reset/hang evidence and its image identity, and does
not contribute values to the comparison ranges. Measured boots reuse only that
readability inventory: known unreadable addresses are recorded with the discovery
as their source and are not probed again. An unknown entry, inconsistent inventory
or unexpected reference reset/hang leaves the measured capture INCOMPLETE.
Every compared calibration projection and register/analog value therefore comes
from the same primary boot at its selected lifecycle point. Comparison math,
reviews, exclusions and the treatment of unreadable registers are unchanged.

The public `cache` module exchanges boot-scoped opaque calibration records and
promotes verified successful full snapshots through the
[host store](../../hil/lab/CALIBRATION_CACHE.md).
