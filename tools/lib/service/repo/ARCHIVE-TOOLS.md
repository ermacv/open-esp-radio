# Repository tooling

Tools are grouped by the contract they own. Blobray is an independent
reusable tool; `xtask` contains this repository's hand-run
commands. A utility does not need its own Cargo package.

| Path | Inputs and result |
| --- | --- |
| [blobray](blobray/README.md) | Captured binary research, reviewed knowledge and bounded concrete comparison |
| [elf](elf/README.md) | The one ELF and archive view: symbols with their aliases by address, sections, segments, relocations with the one RV32 relocation table, relocatable functions and DWARF source locations; nothing else parses `llvm-nm`/`llvm-objdump` output |
| [vendor-provenance](vendor-provenance/README.md) | The `SOURCE` citation grammar, the one relocation-normalized fingerprint of vendor RV32 functions, the provenance registry and its check, and archive revision diffs |
| [riscv/decode](riscv/decode/README.md) | RV32 instruction bytes to typed instructions, lengths and text, for a selected extension set |
| [riscv](riscv/README.md): [riscv/model](riscv/model/README.md), [riscv/program](riscv/program/README.md), [riscv/analysis](riscv/analysis/README.md), [riscv/lift](riscv/lift/README.md) | RV32 program model: identities and contracts, static images, bounded CFG and value analysis, RV32 lifting, and placement-independent function listings (`listing`) that `cargo fw compare` compares |
| [riscv/stack](riscv/stack/README.md) | Worst-case stack bounds of a static RV32 image, failing closed with the unresolved sites by reason; the only stack analyzer, which the image pipeline's stack gate runs |
| [symbol-lineage](symbol-lineage/README.md) | Source function names carried across obfuscated vendor archive revisions |
| [phy-archive](phy-archive/README.md) | Verification of the source-only PHY archive's compiled symbols |
| [process](process/README.md) | Host child-process ownership, cancellation and bounded cleanup; the one Git runner, the checkout a tool acts on (`Checkout`, `built_root`) and live-process facts from `/proc` |
| [durable](durable/README.md) | Atomic file replacement, SHA-256 of files and bytes, wall-clock timestamps and the XDG state directories every tool keeps outside a checkout |
| [toolchain](toolchain/README.md) | The one lookup of Cargo, `rustc`, the toolchain's LLVM tools and `espflash`, the tool versions builds record, and the image compiler setup with the image linker built from the tree being built |
| [image](image/README.md) | Image packages: the pipeline, bundle format, encoding, policy, checks, comparison and linker; `build(ImageSpec) -> ImageBundle` builds staged and ESP-IDF-bootloader images with their gates and source inputs |
| [device](device/README.md) | Board operations in `oer-devices` modules, with image writes behind the `image` feature; independent identity, locking and reference-peer grammar packages |
| `command-tree` | The command tree every repository tool prints for `__command-tree` |
| `chip-profile` | Supported chips resolved from `platform/<chip>/chip.toml`: Rust target (`rust_target`), boot flow, `espflash` chip name, silicon revisions and the flash map (`FlashMap`: offsets, partition tables and how a written image starts, `Start`); the HIL agent's paths belong to [`oer-hil-image-class`](../hil/host/image-class) |
| [repo](repo/README.md) | The one model of the repository: file inventory, manifests, workspaces, chips, typed package classification, the layer/platform/role dependency rules, `owner(path)` and path-package closures |
| [vendor-artifacts](vendor-artifacts/README.md) | The only reader of a chip's pinned vendor artifacts (`verification/<chip>/artifacts.toml`), the host-wide store and fetching into it, for `cargo verification`, the image pipeline's stack gate, the vendor scenarios, host stands' build scripts and the HIL stand's ESP-IDF builds |
| [registers](registers/README.md) | Register model contracts, publication and generated SVD/bindings |
| [tidy](tidy/README.md) | The repository model as text (`cargo tidy workspaces`, `cargo tidy chips`) and the Cargo cache fetch (`cargo tidy fetch`) |
| [xtask](xtask/README.md) | The repository commands run by hand: API docs, firmware locks, worktrees, `push` (a draft pull request) and the `ui` and `feature-sets` tests; domain work stays with its owners |

[HIL](../hil/README.md) owns hardware execution and fixtures;
[vendor projects](../verification/README.md) own investigation composition.
Neither producer decides product readiness. Register model/publication inputs
have a separate [source map](../registers/esp32s31/README.md).

Blobray analyzes inside its own process under cooperative limits.
Register publication uses the [register tool](registers/README.md).
