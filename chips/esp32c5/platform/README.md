# ESP32-C5 platform data

| File | Holds | Read by |
| --- | --- | --- |
| `chip.toml` | the chip layer of the [chip facts](../../espressif/facts/README.md): properties, flash map, address map, harts, revisions, ROM, reset cause; the image registry (`[hil]`, `[packages]`, `[[probe]]`) | `oer-espressif-facts`, `oer-chip-profile` and everything that loads a profile |
| `modules/esp32-c5-wroom-1-n16r8.toml` | the module layer: the flash and PSRAM fitted with the chip | `oer-espressif-facts` |
| `boards/esp32-c5-devkit.toml` | the board layer: the board that carries the module | `oer-espressif-facts` |
| `stack.toml` | the chip's stack policy, which the HIL policy extends | `oer-chip-profile` (`policy`), the image stack check |
| `partitions/applications.csv` | the application partition table | the image pipeline (`chip.toml` `[flash]` names it) |
| `rom-functions.toml` | reviewed summaries of the ROM functions the images call | the stack analysis (`chip.toml` `[rom]`), the provenance check |

The linker scripts are in `chips/esp32c5/layout/linker` and
`chips/espressif/staged-layout/linker`; the boot is described in
[the board's README](../board/README.md).
