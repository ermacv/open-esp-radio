# Host tools

The host-side tools of the repository: one folder per command-line tool,
libraries by layer. They run on the developer's machine, never on a chip.

```text
tools/
  cli/<tool>/        a thin binary: main, argument parsing, a call into its service
    registers/       `cargo registers` (oer-registers-cli)
    fw/              `cargo fw` (oer-fw)
  lib/
    base/            process, durable, command-tree: no knowledge of chips or formats
    data/            one library per data format, its one reader:
                     register-model, register-contracts, register-review,
                     register-bindings, chip-profile (chip.toml), stand-file
    analysis/        none yet: analyses of images and binaries (wave 3), which
                     know no chip and no format
    service/         what a tool does:
                     registers (validation and generation), firmware (images
                     on the stand's boards), lab (the stand: hubs, router, SSH)
    test/layers/     the test that checks the rules below
```

## Layers

Dependencies go only down: `cli → service → analysis, data → base`.

- A CLI depends on its service and on `data` and `base`, never on another
  CLI.
- A service depends on `analysis`, `data` and `base`, and on another service
  only along this order: `firmware → lab`. There is no reverse edge.
- `analysis` depends only on `base` and external crates; `data` on other
  `data` libraries and `base`.
- `tools/` uses only the portable crates of `crates/` (`base`, `radio` and the
  protocols), never a chip crate. Only `lib/service/firmware` uses
  `firmware/`, and only its host-built image layout
  (`platform/espressif/staged-layout`, `platform/esp32s31/layout`); nothing in
  `tools/` depends on `firmware/apps` or on a chip crate of the firmware.
- `crates/` never depends on `tools/`. A crate never sits inside another
  crate's folder.

`lib/test/layers` reads the path dependencies of every `Cargo.toml` below
`tools/` and `crates/` and checks these rules by folder; manifests carry no
layer metadata. `cargo test -p oer-tools-layers` runs it.

There is one workspace, the repository's root; `firmware/` is its own. The
`cargo registers` and `cargo fw` aliases (`.cargo/config.toml`) run the CLIs.
In the firmware workspace only `staged-layout` and the S31 `layout` build on
the host; CI runs their tests (`staged-runtime`'s tests build only for the
chip).
