# Image bundle

`oer-image-bundle` is the format of what one image build makes: a directory
with a `bundle.json` manifest, the runtime ELF, the encoded flash contents and
the gate reports. The crate holds the format only. `oer-image-build` fills and
publishes a bundle, `oer-image-checks` examines the ELF files of one, and a
flash writer (`oer-lab-board`) reads one; none of the readers needs the build
pipeline or its analysis layers.

An `ImageBundle` is one directory with a `bundle.json` and these files:

| File | Content |
| --- | --- |
| `runtime.elf` | The application ELF (the staged boot's stage-two runtime) |
| `application.bin` | The encoded ESP application image (the staged boot's bootstrap with the packed runtime embedded) |
| `bootloader.bin`, `partitions.bin` | The second-stage bootloader and the binary partition table |
| `otadata.bin` | The OTA selection of the first slot (staged only) |
| `runtime.bin`, `bootstrap.elf` | The packed runtime and the bootstrap (staged only) |
| `placement.txt`, `runtime-stack.txt`, `interrupt-stack.txt`, `bootstrap-stack.txt` | Gate reports |
| `bootstrap-placement.txt` | Mandatory cache-off bootstrap code/constant placement |
| `bootstrap-loader.txt` | Encoded mapped-segment count required by the chip's loader, when specified |
| `runtime-frames.txt`, `bootstrap-frames.txt` | All recorded frames against the move limit, exceptions with ceilings and spare bytes, Diagnostic warnings |
| `interrupt-placement.txt` | Interrupt code placement, paths to cached functions, static data references and objects, and free SRAM |
| `image-linker.toml`, `strict-text.x`, `strict-placement.toml` | Runtime linker analysis inputs, generated strict-code fragment and the functions moved with their input sections |
| `large-values.json`, `large-values.txt` | Compiler values above the move limit and workspace coverage |
| `build.log` | Every step's standard error |

The manifest uses schema 7 and records `purpose: Flash` or `Analysis`, the
chip, the boot kind, the flash map, every requested check of every ELF with its
result (`CheckRecord`, `CheckResult`, `CheckedElf`) and the gate warnings. Older
schemas must be rebuilt; a missing purpose never defaults to `Flash`.

## Reading

`ImageBundle::load` verifies every flash file against the digest the manifest
recorded, for either purpose. `ImageBundle::snapshot` reads the flash files
once into owned bytes, verified the same way: what a write hashes into its
receipt and writes over USB or JTAG. The shared snapshot reader refuses
`Analysis` before supplying bytes to any writer, including `cargo fw` and HIL.
`ImageBundle::segments` is what a flash writes: bootloader, partition table and
application at the flash map's offsets, the OTA selection last. Board support
writes these files and encodes nothing. `with_application` makes the bundle of
an application archived earlier around this bundle's boot files; the copy keeps
the original purpose.

## Publication

A bundle is published whole. A builder fills the staging directory
(`staging_directory`: `.<name>.staging` beside the output) and
`ImageBundle::publish` records each flash file's SHA-256 and length in
`bundle.json` and renames the staging directory into place. A build that fails
never publishes, so the bundle published before stays as it was.

Run host regressions with `cargo test -p oer-image-bundle`.
