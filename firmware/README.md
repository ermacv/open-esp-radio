# Firmware

The images for a chip: applications, the bootstrap, HIL agents, probes, peers
and vendor references. The staged boot they share is in `chips/platform/` and
`portable/platform/`, each chip's platform data in `portable/<chip>/platform/`.
The libraries they link are tested on the host.

The boot runs ROM → ESP-IDF bootloader → flash bootstrap → stage two in PSRAM
(see [the ESP32-S31 board](../chips/platform/esp32s31-board/README.md)). The bootstrap embeds
the packed stage-two runtime named by `PSRAM_RUNTIME_BIN`; an empty file is
enough to type-check:

```console
PSRAM_RUNTIME_BIN=/path/to/empty.bin \
  cargo check --target riscv32imafc-unknown-none-elf -p <package>
```

Images live in `apps/`; `boot-smoke` is the smallest. Building, packing,
flashing and running them is the job of [`cargo fw`](../tools/fw/README.md):

```console
cargo fw run boot-smoke --board s31-a
```
