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

The `tracking-hold` kind requests 1–16 forced due ticks of an active diagnostic
154 RX session. `phy-tracking-hold-ieee802154-idle` uses five ticks per boot and
three repetitions. It preserves coarse raw intervals, validates committed
branches, reports per-repetition maxima and requires policy restoration plus
RF closure. Invalid pages or failed cleanup remain INCOMPLETE; no Wi-Fi traffic
or idle hold is served by this workload.

Reference readability is discovered in a separate capture before the alternating
series. Discovery retains reset/hang evidence and its image identity, and does
not contribute values to the comparison ranges. Measured boots reuse only that
readability inventory: known unreadable addresses are recorded with the discovery
as their source and are not probed again. An unknown entry, inconsistent inventory
or unexpected reference reset/hang leaves the measured capture INCOMPLETE.
Every compared calibration projection and register/analog value therefore comes
from the same primary boot at its selected lifecycle point. Comparison math,
reviews, exclusions and the treatment of unreadable registers are unchanged.
