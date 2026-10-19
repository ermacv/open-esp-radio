# ESP32-S31 platform data

| File | Holds | Read by |
| --- | --- | --- |
| `chip.toml` | the chip profile: family, Rust target, boot flow, flash map, revisions, properties, placement contract | `oer-chip-profile` and everything that loads a profile |
| `stack.toml` | the chip's stack policy, which the HIL policy extends | `oer-chip-profile` (`policy`), the image stack check |
| `partitions/applications.csv` | the application partition table | the image pipeline (`chip.toml` names it) |
| `rom-functions.toml` | reviewed summaries of the ROM functions the images call | the stack analysis (`chip.toml` `[rom]`), the provenance check |

The linker scripts are in `portable/platform/esp32s31-layout/linker` and
`portable/platform/staged-layout/linker`; the boot is described in
[the board's README](../../../chips/platform/esp32s31-board/README.md).
