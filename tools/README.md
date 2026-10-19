# Host tools

The programs of the repository that run on the developer's machine: one
folder per program, its `main.rs` and its own modules. Their libraries live
in `host/` and `portable/`; the layout and its rules are
[docs/repository-layout.md](../docs/repository-layout.md).

```text
tools/
  registers/       `cargo registers` (oer-tool-registers): validate the register model, generate SVD, PAC and bindings
  fw/              `cargo fw` (oer-tool-fw): build, flash and run staged images and probe images
  blobray/         `blobray`: binary analysis of images and vendor libraries
  pins/            `cargo pins` (oer-tool-pins): fetch the vendor pins
  stand/           `cargo stand` (oer-tool-stand): boards, leases, power, fixtures
  hil/             `cargo hil` (oer-tool-hil): HIL runs, the run store and its analyses
  verification/    `cargo verification` (oer-tool-verification): the vendor comparison
  vendor-scenarios-<chip>/  each chip's typed vendor scenarios
```

## Rules

A program depends on `host/` and `portable/` only, never on another program
or on `chips/`. `host/test/layers` reads the path dependencies of every
package manifest and checks the root rules of the layout by folder; manifests
carry no layer metadata. `cargo test -p oer-test-layers` runs it.

A package is named after its folder, with no `[lib] name` override, and the
layer test checks it: a library is `oer-` and its path below its root
(`host/image/build` is `oer-image-build`), a program keeps its root
(`tools/fw` is `oer-tool-fw`), and so does an image (`firmware/apps/w1` is
`oer-firmware-apps-w1`). Code uses the same name with underscores.

There is one workspace, the repository's root, with one `Cargo.lock`. Its
`default-members` are every package but the chip-only ones (`firmware/` and
the few packages of `chips/` and `portable/` the layer test lists); the layer
test checks that list. The images build with `--profile firmware` for the
chip target (`cargo fw`). The aliases in `.cargo/config.toml` run the
programs.
