# Examples

Applications on the application entry `oer`: each names the capabilities it
uses as features of `oer` and no chip. The image pipeline selects the
board's chip, builds the image for it and links it with the chip's staged
boot (ROM → ESP-IDF bootloader → flash bootstrap → stage two in PSRAM on the
ESP32-S31).

```console
cargo fw build wifi-station
cargo fw run wifi-station --board s31-a
```

A type-check names the chip itself; the bootstrap embeds the packed
stage-two runtime named by `PSRAM_RUNTIME_BIN`, and an empty file is enough:

```console
PSRAM_RUNTIME_BIN=/path/to/empty.bin \
  cargo check --target riscv32imafc-unknown-none-elf -p oer-examples-wifi-station --features oer/esp32s31
```

| Example | What it does |
| --- | --- |
| [`wifi-station`](wifi-station/README.md) | Joins its build profile's network, takes an address by DHCP and exchanges a line over TCP with the gateway |
