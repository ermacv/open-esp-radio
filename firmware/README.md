# Firmware

The chip-target workspace for the ESP32-S31: the staged boot shared by the
Espressif chips (`platform/espressif`), the ESP32-S31 board, bootstrap and
address map (`platform/esp32s31`) and, from the first slice on, the images.
The radio libraries it links live in the host workspace and are tested there.

The boot runs ROM → ESP-IDF bootloader → flash bootstrap → stage two in PSRAM
(see [platform/esp32s31](platform/esp32s31/README.md)). The bootstrap embeds
the packed stage-two runtime named by `PSRAM_RUNTIME_BIN`; an empty file is
enough to type-check:

```console
PSRAM_RUNTIME_BIN=/path/to/empty.bin \
  cargo check --target riscv32imafc-unknown-none-elf -p <package>
```

Images live in `apps/`; `boot-smoke` is the smallest. Building, packing,
flashing and running them is the job of [`cargo fw`](../host/image/build/FW.md):

```console
cargo fw run boot-smoke --board s31-a
```
