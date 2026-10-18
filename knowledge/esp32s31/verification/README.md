# Vendor verification

This tree holds the evidence that the production drivers follow the pinned
vendor code, organized by level. The table names the ESP32-S31 owners; the
[ESP32-C5 project](esp32c5/README.md) has the same structure. It contains no production driver behavior,
no HIL board scenarios and no private vendor artifacts.

`verification/Cargo.toml` owns the host CLI, harness, evidence producers and
chip scenarios. Firmware probes and host stands use independent workspaces
below their chip directory. Root-workspace shared support libraries live with
their consumers: the [shard format](../qualification/evidence-shard/README.md)
in `qualification/`, and the [PHY capture](../hil/phy/capture/README.md),
chip calibration relations and comparisons in `hil/phy/`.

| Level | Claim | Where |
| --- | --- | --- |
| L0 pins | which vendor code is the reference | [`esp32s31/artifacts.toml`](esp32s31/artifacts.toml) |
| L1 facts | recovered constants, tables and register facts describe the pinned code | production `SOURCE:` blocks, [`registers`](../registers/README.md) evidence, [`esp32s31/facts`](esp32s31/facts) |
| L2 behavior | compiled production code behaves as the vendor code | [`esp32s31/probes`](esp32s31/probes/README.md), [`esp32s31/scenarios`](esp32s31/scenarios), [`esp32s31/host`](esp32s31/host/ieee802154/README.md); `cargo verification evidence` computes their derived index into `target/verification/esp32s31/evidence` |
| L3 hardware | the drivers work on the board and calibrate as the vendor firmware does | [`hil`](../hil/README.md), whose cited runs the pinned evidence archive holds, the [ESP32-S31 calibration comparison](../hil/phy/esp32s31/calibration/README.md) with [`esp32s31/hil-vendor`](esp32s31/hil-vendor/README.md) |
| L4 readiness | a capability is qualified | [`qualification`](../qualification/README.md) |

```text
verification/
  cli/               oer-verification-cli: the host command line
  evidence/          oer-vendor-evidence: evidence producers and orchestration,
                     the verdict source policy and the producer contract;
                     re-exports the root workspace's shard format
  harness/           probe code generation and macros, and the chip-neutral
                     scenario engine (harness/scenarios)
  esp32s31/
    artifacts.toml   L0: every pinned archive, ROM ELF and SDK build
    facts/           L1: cited-function fingerprints, recovered name maps
    probes/          L2: isolated workspace of compiled production entries
    scenarios/       L2: typed Blobray comparisons against the pinned binaries
                     (the verdict library; cli/ is its binary)
    host/ieee802154/ L2: the public IEEE 802.15.4 driver compiled on the host
    evidence/        the L2 shards (scenarios/) once generated; no shard
                     is tracked at present
    hil-vendor/      L3: vendor ESP-IDF firmware for hardware cross-checks
    hardware/        L3: isolated firmware probes of board state
  esp32c5/           the same levels for the ESP32-C5: pins, facts, probes,
                     scenarios and a board register probe
```

**L0.** `cargo verification fetch esp32s31` downloads and verifies every pinned
artifact into `target/vendor/`; scenarios default to those paths and reject
any other bytes. Changing a pin means changing the manifest, then following
the pinned behavior in production.

**L1.** A recovered fact names the vendor function it was read from. `cargo
verification check provenance --chip esp32s31` fails when such a function changed since its facts
were reviewed; `cargo verification diff --chip esp32s31` shows what changed between two pins,
and `tools/symbol-lineage` pairs obfuscated names across releases. See the
[source policy](../docs/source-policy.md).

A function both a pinned ROM and a pinned library define has two copies,
which can handle the same register bits differently, and a fingerprint check
of each copy does not see it. `cargo verification copies --chip <chip>`
compares every such pair by its MMIO behavior inside the register model's
MMIO windows, each ROM revision on its own: against a built vendor firmware
that links the function (every library address resolved there, with the
chip's flash as read-only), else against the archives. Each pair gets a
verdict, strongest first: `differs` (both analyses complete and a write
differs), `stored-value-unknown` (the same words written, one side's bits
unresolved), `several-copies` (a side holds several copies of the name, whose
accesses compare only as one multiset), `reads-differ` (only loads or read
bits, which equivalent code can describe differently), `differs-incomplete`,
`unresolved-differs`, `unknown` (a copy blocked, or in code the analysis
skips) and `no-difference-seen`, which is no proof of equality; each pair
says whether each side's analysis of it is complete. The report lists the copies our facts cite and is written to
`target/verification/<chip>/copies.json`; unbuilt vendor firmware is named,
since its functions fall back to the noisier archive comparison.

**L2.** A typed scenario runs the pinned vendor function and the compiled
production probe in one [Blobray](../tools/blobray/README.md) session and
fails closed with MATCH, DIFF or INCOMPLETE. Each scenario writes its shard
of the evidence index with `--index`. The index is derived data: every shard
follows from the checkout's sources and pins, so nothing tracks it.
`cargo verification evidence --chip <chip>` computes it for the checkout into
`target/verification/<chip>/evidence`, fetching the pins and building the
firmware inputs from their recipes first, and fails when a scenario does not
match; without `--chip` it computes every verified chip's index. On a
branch, `--compare <directory>` sets an index computed earlier (the merge
base's) beside the branch's. The comparison fails when a vendor root
the base index claims is claimed by no shard of the branch's (a claim
exists only with a MATCH), or when the branch's comparisons of a claimed
root reach fewer of its vendor closure's blocks or branch directions than
the base's (the most any of the root's entries reach; a root whose base
entry records coverage in a form this one does not read has an unknown
reach, so only its loss compares, and the comparison says so). It reports every
other difference (a root's production entry or scenario, cases, coverage
totals, observation, state, untriaged locations) without failing, since
refactors and analyzer improvements move those. A loss on purpose is
accepted with its reason in
`verification/<chip>/decisions/accepted-losses.toml`: a `[[retired]]` root
the index no longer claims, or a `[[narrowed]]` root with the `blocks` and
`directions` its comparisons keep, so a further narrowing fails again.
Retiring a root the index still claims, accepting a narrowing of one it
does not claim, or keeping a narrowing its comparisons reach beyond again,
is an error; an acceptance may go once `main` holds it.
Qualification reads the index from there; an index a source or reviewed
coverage decision changed since holds no evidence, never partly trusted
evidence, and its report names it stale until it is computed again. The [ESP32-S31 project](esp32s31/README.md)
describes the scenarios, their inputs and their reviewed decisions; the
[vendor contract reference](../docs/vendor/esp32s31/README.md) explains the
vendor behavior they cover.

**L3 and L4.** HIL runs exercise the drivers on hardware; `cargo hil firmware
build <image>` builds each vendor firmware of `hil-vendor/` against
the pinned ESP-IDF, with its PHY, coexistence, Wi-Fi and Bluetooth library
submodules replaced by the pinned archives, and the
[calibration cross-check](../hil/phy/esp32s31/calibration/README.md) compares
its cold calibration with production's on the same board through the
tracking scenario's reviewed relation. No HIL scenario captures both sides
now; the scenarios return with the HIL architecture of #761. A run that
evidence cites enters the pinned HIL evidence archive. The only path from
comparison or HIL evidence to product readiness is the independent
[qualification evaluator](../qualification/README.md).

Private inputs never enter the repository: `_oracles/` and fetched artifacts
stay ignored, and checked configuration never names machine-local paths.
Generated analysis stays in ignored outputs.
