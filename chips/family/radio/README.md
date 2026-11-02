# Shared radio domain drivers

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
The family's `membership` driver owns first/last common-power membership,
retained baseline commit, retry order, one-time Wi-Fi reset and coexistence
participant accounting. `epoch` retains the packed registration counter across
route retirement. `domain` checks BTBB registration/gain generation before its
first initialization, orders low-power selector/reference edges, and sequences
external coexistence. `modem` retains each module lease and every poisoned
planner transaction through refusals.

The HAL's S31 binding retains public affine client/registration/coexistence
proofs, clock guards, snapshots, register owners and PHY transactions. Its
methods lend those atoms and owners to the family driver and translate typed
results. Entry ports borrow a provider; release ports require only the held
guards. The family obtains no register handle or reset authority. Exact affine
reunion, both quarantine guards and the cancellation reporter ABI remain HAL
custody obligations. Chip PHY and radio-system lifecycle run above this binding.

Recording fake ports use `oer-time-virtual` to check effect order, elapsed atom
time, partial failures, first/last clock edges and permanent-owner retention.
HAL tests separately check register atoms and ownership/UI constraints.
