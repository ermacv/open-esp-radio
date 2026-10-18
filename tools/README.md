# Host tools

The host-side tools of the repository: one folder per command-line tool,
libraries by layer. They run on the developer's machine, never on a chip.

```text
tools/
  cli/<tool>/        a thin binary: main, argument parsing, a call into its service
    registers/       `cargo registers` (oer-tool-cli-registers)
    fw/              `cargo fw` (oer-tool-cli-fw)
    blobray/         `blobray`: binary analysis of images and vendor libraries
    pins/            `cargo pins` (oer-tool-cli-pins): fetch the vendor pins
  lib/
    base/            process, durable, command-tree, toolchain: no knowledge of
                     chips or formats
    data/            one library per data format, its one reader:
                     register-model, register-contracts, register-review,
                     register-bindings, chip-profile (chip.toml), stand-file,
                     vendor-pins and vendor-artifacts (artifacts.toml and the
                     store), image-policy, markers (`SOURCE:` blocks)
    analysis/        analyses of images and binaries, which know no chip:
                     elf, riscv-* (decode, lift, model, program, analysis,
                     stack) and the Blobray engine (blobray-*)
    service/         what a tool does:
                     registers (validation and generation), firmware (images
                     on the stand's boards), lab (the stand: hubs, router, SSH),
                     image-check-* (stack, interrupt stacks, placement),
                     image-linker (the post-link input check), vendor-provenance
                     (the `SOURCE:` registry) and symbol-lineage
    test/layers/     the test that checks the rules below
```

## Layers

Dependencies go only down: `cli → service → analysis, data → base`.

- A CLI depends on its service and on `data` and `base`, never on another
  CLI.
- A service depends on `analysis`, `data` and `base`, and on another service
  only along an edge `lib/test/layers` lists (`SERVICE_ORDER`): the stand
  and HIL services build on each other, and each new edge is added there on
  purpose. There is no cycle.
- `analysis` depends on other `analysis` libraries, `base` and external
  crates; `data` on other `data` libraries and `base`.
- `tools/` uses only the portable crates of `crates/` (`base`, `radio` and the
  protocols), never a chip crate. Only `lib/service/firmware` uses
  `firmware/`, and only its host-built image layout
  (`platform/espressif/staged-layout`, `platform/esp32s31/layout`); nothing in
  `tools/` depends on `firmware/apps` or on a chip crate of the firmware.
- `crates/` never depends on `tools/`. A crate never sits inside another
  crate's folder.

`lib/test/layers` reads the path dependencies of every `Cargo.toml` below
`tools/` and `crates/` and checks these rules by folder; manifests carry no
layer metadata. `cargo test -p oer-tool-test-layers` runs it.

A package is named after its folder, with no `[lib] name` override, and the
layer test checks it: `crates/a/b` is `oer-a-b` (`crates/esp32s31/ieee80211/driver`
is `oer-esp32s31-ieee80211-driver`), `firmware/a/b` is `oer-firmware-a-b`
(`firmware/apps/w1` is `oer-firmware-apps-w1`), and below `tools/` the name is
`oer-tool-` and the path without the `lib/` level (`tools/lib/service/firmware`
is `oer-tool-service-firmware`, `tools/cli/fw` is `oer-tool-cli-fw`). Code uses
the same name with underscores. When a clearer name needs another folder, the
folder moves.

There is one workspace, the repository's root, with one `Cargo.lock`. Its
`default-members` are every package but the chip-only ones of `firmware/`
(`staged-layout` and the S31 `layout` build on the host and are default
members); the layer test checks that list. The images build with
`--profile firmware` for the chip target (`cargo fw`). The `cargo registers`
and `cargo fw` aliases (`.cargo/config.toml`) run the CLIs.
