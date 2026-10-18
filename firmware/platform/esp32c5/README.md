# ESP32-C5 staged application boot

This platform composes ESP-HAL for the ESP32-C5 devkit with the
ESP32-C5-WROOM-1-N16R8 module (16-MiB quad flash, 8-MiB quad PSRAM). It
boots like the [ESP32-S31](../esp32s31/README.md), on the shared crates of
[`platform/espressif`](../espressif/staged-layout/README.md): ROM → ESP-IDF
bootloader (espflash's, DIO) → Flash bootstrap (QIO, 80 MHz) → stage two in
PSRAM.

| Component | Responsibility |
| --- | --- |
| `layout` | The board's `LAYOUT`: SRAM below the second-stage loader at `0x4084e5a0`, the flash half of the shared cache window, PSRAM fixed at `0x43000000`, the panic record in LP RAM at `0x50000000`, one hart |
| `board` | Quad PSRAM at 80 MHz mapped at the fixed origin; stage two's adoption of the mapping (`stage-two`) |
| `bootstrap` | The shared bootstrap steps; no Flash tuning |
| `linker/rom` | The ECO2 ROM symbols (revision v1.0) and the reviewed ROM function summaries |
| `partitions` | Application partition layout |
| `stack.toml` | Stack policy; the coverage review is shared (`../espressif/stack-coverage.toml`) |

The chip has one MMU whose 32-MiB window serves flash and PSRAM; esp-hal
maps PSRAM after the last flash page unless asked otherwise, so the board
maps it at a fixed page (`PsramOrigin::Fixed`) at which stage two is linked.
The runtime enables no FPU (`riscv32imac`). The bootstrap enables no modem
clock: see the ESP32-C5 entry of the [hardware errata](../../docs/hardware-errata.md).

## Build

esp-hal comes from the owner's fork, branch `oer/pr313-bod-mode1-fib`
(revision `4fd4ba9b`, on top of `oer/pr210-panic-sram`): fixed-origin PSRAM mapping (`PsramOrigin::Fixed`), code
preparation in PSRAM and re-initialized interrupt vectoring after the
handoff on the ESP32-C5. The shared panic handler records into retained
RAM and resets without formatting. HAL restores UART0's undivided XTAL
clock directly, so reset cannot reacquire a clock-tree lock held at panic;
chip register access stays in HAL. The shared linker places panic entry
helpers and the record/reset path in SRAM, including outlined HAL reset
preparation. HIL hooks also keep their code, state and table-free post-mortem
CRC in uncached memory; interrupt-context panic text is omitted because the
compiler may store it in PSRAM. The final-image gate rejects cached callees
of panic entries and hooks. Images are built by the
[image pipeline](../../tools/image/pipeline/README.md) from `chip.toml` (boot
`staged`, the `[flash]` map: application QIO at 80 MHz on 16 MiB, DIO
bootloader). The HIL agent (`hil/targets/esp32c5/agent`) runs on the staged
runtime; its image classes are `boot-smoke` and `system-watchdog`:

```console
cargo hil images check --chip esp32c5 --all --type-check
cargo hil image build boot-smoke --chip esp32c5
cargo hil run --chip esp32c5 boot-smoke system-watchdog
```

The [HIL scenarios](../../hil/targets/esp32c5/README.md) exercise the staged
handoff, timer wake and watchdog resets on the stand. Flash and PSRAM stay
at 80 MHz; 120 MHz needs esp-hal's MSPI timing tuning for this chip.
