# Versioned scenario catalog

Folders identify the workload domain: system, IEEE 802.11 and PHY. PHY investigations
alternate the pinned Wi-Fi or IEEE 802.15.4 reference and the diagnostic calibration image,
or measure forced tracking intervals with 802.15.4 receiving in idle.
Scenario IDs, workloads, image classes, profiles and checks use names that
describe their behavior. References to design decisions state the decision
directly. Scenario IDs are stable across folder moves. Tags select overlapping
diagnostic and characterization uses; they do not assign ownership or make a
hardware-readiness claim.

## Roles

Every document declares its `role`. A `qualification` scenario is one that a
qualification program references; every other scenario is an `investigation`
scenario: the catalog of diagnostics, measurements and experiments no program
requires. A scenario on a diagnostic image class (`diagnostic-*`), or one that requests a
profile, must be an `investigation` scenario and may not carry the
`performance` tag: observers and probes the product does not carry never
qualify it or shape a gated figure. `cargo hil run-all --role qualification`
runs every qualification scenario.

## Document shape

Schema 5 documents have a common header and exactly one family table:

```toml
schema = 5
id = "boot-smoke"
description = "..."
role = "investigation"          # or "qualification"; see Roles
repetitions = 3                 # default 1
tags = ["smoke"]
unsupported = "..."             # optional; see Unsupported scenarios

[system]
kind = "boot-smoke"
```

The family owns every executable value:

- `[system]` is a tagged workload whose kind implies the firmware image.
- `[lower-mac]` runs conformance and ownership-close checks.
- `[wifi]` runs WPA2-Personal station qualification.
- `[phy]` runs the vendor calibration cross-check or the quiet Wi-Fi/154 idle tracking-hold
  investigation on `diagnostic-phy`.
Every table rejects unknown fields. Relations that remain between the image,
the data path and the workload are validated by the family.

## Chips

A scenario names no chip. It runs on every chip whose HIL agent builds its
image class: the esp32s31, whose staged boot flow builds every class its
runtime declares. A run takes the chip
`cargo hil run --chip CHIP` names, or the only chip that builds every selected
image; when several can, it refuses and names them. It uses that chip's device
under test from the stand file.

## Stand claims

A run claims the boards and fixtures its scenarios require and the frequency
ranges their radio work occupies. The range follows the family: system
scenarios occupy the 2.4 GHz band, and the radio-free watchdog none. By default a
scenario tolerates other protocol traffic in its range and transmits
normally. Tags change that:

| Tag | Meaning |
| --- | --- |
| `air-strict` | No other transmitter may use its range: RF levels, noise, sensitivity, A/B throughput |
| `air-noisy` | It transmits without regard for others: a continuous carrier, no CSMA, DTM, a throughput flood |
| `air-exclusive` | Both of the above |

Two runs conflict only where their ranges overlap: a strict run excludes any
transmission there, and a tolerant one excludes a noisy transmitter. Work on
disjoint ranges, or without a radio, runs in parallel. Every run records the
ranges it held, their level and the other leases held at its grant in
`air.json`. Tags are excluded from scenario digests, so adding a tag does not
invalidate recorded evidence.

## Named checks

Named checks are derived from implemented workload behavior and explicit
criteria, not tags. `cargo hil plan --proof NAME` filters candidates. It never fills in missing
observations from a suite PASS.

The runner discovers regular TOML files recursively. The filename stem must equal the document ID. IDs are
unique throughout the catalog. README.md is the only ignored documentation
filename. Symlinks (including a symlink catalog root), special files, other
file extensions and an empty catalog are rejected. One discovery,
`oer_hil_scenario::catalog::documents`, never follows directory links outside
the catalog and checks every header (schema, id, repetition bounds) and the
single family table; the runner interprets executable workload and acceptance
fields.

The runner sorts by scenario ID.
`run-all` first traverses `ImageClass::ALL`, then the selected
scenarios of each image in catalog order. Folder traversal order cannot change
physical execution order. Each TOML document owns its image features,
workload criteria and repetition count.

Synthetic serialized compatibility inputs live in `tools/hil/run/tests-fixtures/fixtures/catalog`.
Both readers' tests use them; they are not part of this catalog.

## Program-counter profile

An optional top-level `[profile]` table (`harts = "both" | "core0" |
"core1"`, `period-us` in 100..=100000) samples the program counter of the
image's harts during the workload's measured window. No image has the sampler
(`pc-profile`), so the runner refuses every profile request. Sampling perturbs timing; the
scenario validation also restricts diagnostic profiles to investigations
without the `performance` tag. The table is part of the procedure, so a profiled scenario
is a different scenario from its unprofiled copy, and its evidence never
stands in for a throughput or timing measurement.

## Unsupported scenarios

The optional top-level `unsupported` field states why the current firmware
cannot run a scenario: no image it builds serves the workload's image class,
or the class's image does not declare a role the workload drives. The runner
refuses an explicitly named unsupported scenario before building any image;
a tag or run-all selection skips it and prints its id and reason once. The field is not part of the executed procedure, so
recorded evidence stays comparable. The runner's catalog test fails when a
marked scenario is served, or an unmarked one is not, by the image classes
declared in `firmware/hil/esp32s31/Cargo.toml` and their declared
capabilities; after flashing, the runner checks the same roles against the
capabilities the image reports.
