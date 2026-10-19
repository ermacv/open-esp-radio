# Host toolchain

`oer-os-toolchain` is the one place repository tools locate the host programs
builds run and record which ones ran.

- `program(Tool)` / `command(Tool)`: a non-empty override variable names the
  program (`CARGO`, `RUSTC`, `ESPFLASH`, `LLVM_NM`, `LLVM_OBJCOPY`,
  `LLVM_OBJDUMP`). Otherwise Cargo, `rustc` and `espflash` come from `PATH`,
  and the LLVM tools from the `llvm-tools` component of the active Rust
  toolchain (`<sysroot>/lib/rustlib/<host>/bin`, the toolchain
  `rust-toolchain.toml` pins), never from an unrelated LLVM on `PATH`.
- `require(Tool)` fails unless the tool runs; `versions()` lists every tool
  with the program and the version it reports (`llvm-objdump` included).
- The image compiler setup is the image pipeline's
  (`oer_image_build::compiler`).
- `host_target()` names the host's target triple, for a workspace whose
  Cargo configuration defaults to a chip target.
- `blobray::{cargo, binary, host}`: Cargo commands in the separate Blobray
  workspace with its own target directory, and the built `blobray` host, as
  every caller that builds or runs Blobray from the root workspace (the
  vendor scenarios, the register inventory, the conformance check) reaches them.

It depends only on `oer-os-process`.

```console
cargo test -p oer-os-toolchain
```
