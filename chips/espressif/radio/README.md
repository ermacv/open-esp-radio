# Shared radio power and clocks

`oer-espressif-radio` executes common power prerequisites, checks the semantic
readback, restores the cold-power baseline in three verified stages, and
counts platform clock references above a retained permanent memory owner.
It is generic over register and reference-storage ports and contains no chip,
PAC, runtime, protocol driver, reset or unsafe dependency.

`power::PowerSequenceBackend` takes MPLL before PMU publication, opens the bus
and maps, takes the 160 MHz reference before selecting the source, pulses only
Wi-Fi baseband reset, enables calibration clocks and selects PHY I2C before
its master reference. All readbacks are sampled after the final edge and checked
in their original order. A refused reference or readback names its checkpoint;
the caller retains previously acquired references.

`baseline::PowerBaselinePort` lends an opaque chip snapshot. Restore verifies
syscon, source and bus in order, returning before the next write on disagreement.
The caller retains its snapshot until success. Monotonic ICG maps are excluded.

`clocks::ClockCounts` commits only after an edge. Overflow and an unmatched
release return typed errors without writes. The S31 binding keeps its permanent
`PsramMpll` in this owner, and its MPLL gate atom performs no write.
The HAL retains the single clock-word claim and short exclusive loans.
The family also selects Wi-Fi XTAL32K only for that RTC source; every other
RTC selector, including invalid values, retains the existing RC_SLOW selection.

The S31 modem tables live in `knowledge::modem_clock`; its HAL newtype binds
them to `oer-espressif-modem-clock`. That family's `execution` module orders
upstream and device edges and retains the planner's poison/lease semantics.
The HAL's radio custody still owns common-power membership and PHY transactions.
It releases only through exact affine reunion, preserving quarantine and the
existing cancellation reporter ABI.

Recording fake ports use `oer-time-virtual` to check effect order, elapsed atom
time, partial failures, first/last clock edges and permanent-owner retention.
HAL tests separately check register atoms and ownership/UI constraints.
