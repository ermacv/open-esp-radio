# Repository layout

**Status: proposal for the owner's approval.** Nothing below is moved yet;
the last section lists what moves.

The repository holds code that runs on the chip, code that runs on the
developer's machine, and small contracts between the two. A reader should
tell from a path alone which of the three a package is, what it may depend
on, and where its input comes from.

The shape follows the Rust projects that solve the same split:

| Project | Device side (`no_std`) | Shared contract | Host side | Who builds the firmware |
|---|---|---|---|---|
| defmt | `defmt`, `defmt-macros`: a macro writes records into an ELF section | `defmt-parser`: the format both sides use | `defmt-decoder` (library), `defmt-print` (CLI) | cargo; the decoder takes an ELF file and a byte stream |
| embedded-test | `#[test]` writes the test list into an ELF section | the section format | `probe-rs run` reads the section and runs the tests | cargo, with probe-rs as its runner |
| postcard-rpc | server | an ICD crate: types and endpoints only | client | — |
| probe-rs | — | `probe-rs-target`: target data | `probe-rs` (library), `probe-rs-tools` (CLI) | cargo; the tool takes an ELF path |
| espflash | — | — | `espflash` takes a file | a separate thin `cargo-espflash` builds first |

## Three kinds of package

| Kind | Runs on | Lives in | Holds |
|---|---|---|---|
| **Device** | the chip (and the host in tests) | `crates/` except `crates/contract/` | protocols, drivers, HAL, PAC, executors, adapters |
| **Contract** | both | `crates/contract/` | types and wire or section formats shared by a firmware image and a host tool; no behavior of either side |
| **Host** | the developer's machine | `tools/` | analysis, the stand, HIL, register generation, vendor comparison |

`firmware/` holds images: each is a thin binary over device crates and
contracts. `registers/` and `knowledge/` hold data, not code.

## Tree

```text
crates/                      device code; builds for the chip and for host tests
  base/ exec/ radio/         no chip, no protocol
  ieee80211/ bluetooth/ ieee802154/   portable protocols
  espressif/                 Espressif IP shared by their chips
  esp32s31/ esp32c5/         one chip: PAC, HAL, PHY, drivers
  adapters/                  bindings to Embassy and esp-hal
  contract/                  no_std contracts between an image and a host tool
    hil-protocol             the HIL runner's wire protocol
    image-keys               the keys an image announces
    probe                    probe declarations and the probe catalog section
    trace                    trace records (today crates/base/trace)
firmware/                    images, built only by `cargo fw`
  apps/ hil/ verification/ platform/
  peers/ vendor/             ESP-IDF projects, built by `cargo fw` through esp-idf
tools/                       host only; takes artifacts, never builds an image
  lib/base/                  processes, files, toolchain
  lib/data/                  one reader per data format
  lib/analysis/              ELF, RV32, Blobray; knows no chip
  lib/<domain>/              stand, hil, image, vendor, phy, registers
  lib/build/                 build-time tools of images (proc macros, catalog codegen)
  cli/<tool>/                thin binaries: argument parsing and one call
registers/ knowledge/        data
```

## Dependency rules

The layer test checks each rule by folder.

1. **Contracts** are `no_std`, depend only on external `no_std` crates and
   `crates/base`, and build for every chip target in CI. A contract may carry
   a `std` reader of its own format behind a feature (as `defmt-parser` does),
   never host logic.
2. **Device crates** depend downward inside `crates/` (`base` → `radio` →
   protocols → `espressif` → chip → `adapters`). A product crate depends on
   `crates/contract/probe` only under `cfg(oer_probe)`
   (`[target.'cfg(oer_probe)'.dependencies]`), so a product image does not
   link it.
3. **Firmware** depends on `crates/` and `tools/lib/build/` only.
4. **Host tools** depend downward: `cli → <domain> → analysis, data → base`.
   A domain depends on another along a short listed order
   (`hil → stand → lab`, `vendor → image`). Host tools use contracts and
   portable device crates; a chip crate only along the listed vendor
   comparison edges.
5. **Nothing depends on `tools/` from `crates/`.**
6. A package is named after its folder (unchanged).

## Artifacts, not builds

`cargo fw` is the one builder of images: our apps, HIL agents, probe images
and, through `esp-idf`, the vendor peers and references. It writes each
image and a manifest (chip, image, profile, ELF, flash segments, hashes)
below `target/fw/`.

Every other tool takes artifacts as input: an ELF or binary path, a manifest,
or a byte stream from a board. A tool that offers "build and run" in one
command is a thin wrapper that calls `cargo fw` and then the tool, the way
`cargo-espflash` wraps `espflash`.

```text
cargo fw build/probe ──► target/fw/<image>/{image.elf, manifest.json}
                               │
        ┌──────────────────────┼──────────────────────┐
        ▼                      ▼                      ▼
  blobray, image checks   vendor scenarios       hil runner
  (ELF path)              (probe ELF path)       (manifest, board stream)
```

## Scenarios

| I want to | Command | Path through the tree |
|---|---|---|
| build and run an app on a board | `cargo fw run w1 --board s31-a` | `firmware/apps/w1` → `tools/cli/fw` |
| run a HIL scenario | `cargo fw build …`, then `cargo hil run --images <manifest>` | agent `firmware/hil/*` speaks `crates/contract/hil-protocol`; `tools/lib/hil` reads the same contract |
| add a HIL message | edit `crates/contract/hil-protocol` | both sides rebuild against one definition |
| compare a HAL leaf with the vendor | `cargo fw probe <package>`, then `cargo vendor-scenarios <chip> <scenario> --probe <elf>` | hook under `cfg(oer_probe)` in the HAL; `probe!` from `crates/contract/probe`; the host reads the catalog through the same contract |
| check an image's stack and placement | `cargo fw build <image>` (the checks run on its ELF) | `tools/lib/image` reads the ELF the build wrote |
| analyse a vendor library | `cargo pins fetch`, then `blobray … <path>` | Blobray takes files from the pin store |
| add a register field | edit `registers/`, then `cargo registers generate` | the generated PAC in `crates/<chip>/pac` |

## What moves

| Today | Proposed | Why |
|---|---|---|
| `tools/lib/data/hil-protocol`, `hil-image-keys` | `crates/contract/hil-protocol`, `image-keys` | they run on the chip; `no_std` is checked by a chip build |
| `crates/base/trace` | `crates/contract/trace` | it is a device↔host contract |
| `tools/lib/service/probe-macros` and the catalog format in `probe-codegen` | `crates/contract/probe` (format, `probe!`, host reader) and `tools/lib/build/probe-codegen` | one definition of the catalog; today the reader in `vendor-scenario-engine` parses the section on its own |
| `tools/lib/analysis/phy-relation-esp32s31` | a module of `crates/esp32s31/phy` | chip knowledge, used by a probe and by host tools |
| `tools/lib/service/*` (62 packages, 125 listed edges) | `tools/lib/<domain>/`, small packages merged into modules | the domains become visible; the edge list shrinks to the order between domains |
| `image`, `image-bundle`, `image-encode`, `image-compare`, `image-checks` beside `service/firmware` | one image domain behind `cargo fw` | one image pipeline |
| `hil-runner` building images (`execution/firmware.rs`) | `hil-runner` takes a manifest | tools take artifacts |
| the vendor scenario engine finding probe images by convention | an explicit probe ELF path | tools take artifacts |
| 12 packages reading ELF through `object` beside `analysis/elf` | `analysis/elf` | one ELF reader |
| `cli/blobray` (logic and tests in the binary), `cli/hil`, `cli/stand` | logic in their libraries | thin CLIs |
| `vendor-evidence`, `vendor-evidence-shard` (their consumer, qualification, is gone) | removed, unless a vendor tool still needs them | no orphan formats |

## Open questions

- `crates/contract/` or a fourth top-level `contract/`: the proposal keeps
  "code that runs on the chip lives in `crates/`".
- Whether `hil-source-snapshot` and the replay keep a role once the HIL
  runner takes manifests.
