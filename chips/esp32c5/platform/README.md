# ESP32-C5 platform data

| File | Holds | Read by |
| --- | --- | --- |
| `chip.toml` | the chip profile: family, Rust target, boot flow, flash map, revisions, properties, placement contract | `oer-chip-profile` and everything that loads a profile |
| `stack.toml` | the chip's stack policy, which the HIL policy extends | `oer-chip-profile` (`policy`), the image stack check |
| `partitions/applications.csv` | the application partition table | the image pipeline (`chip.toml` names it) |
| `rom-functions.toml` | reviewed summaries of the ROM functions the images call | the stack analysis (`chip.toml` `[rom]`), the provenance check |

The linker scripts are in `chips/esp32c5/layout/linker` and
`chips/espressif/staged-layout/linker`; the boot is described in
[the board's README](../board/README.md).
