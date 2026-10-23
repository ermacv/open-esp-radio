# Memory and stacks

Entry points for the staged images' memory layout, placement and stack
contracts; details live with their owners.

## Regions

The [shared layout](../chips/espressif/staged-layout/README.md) defines the
regions; the [S31 board map](../chips/esp32s31/layout/src/lib.rs) and
[C5 board map](../chips/esp32c5/layout/src/lib.rs) supply their addresses
and sizes.

| Region | Current use and source |
| --- | --- |
| SRAM | Strict and hot code, interrupt stacks, critical state and DMA storage: [runtime sections](../chips/espressif/staged-layout/linker/runtime/sections.x). |
| PSRAM | Ordinary runtime code, data and the CPU0 task stack: [runtime sections](../chips/espressif/staged-layout/linker/runtime/sections.x). |
| Flash XIP | The bootstrap's ordinary code and constants: [bootstrap sections](../chips/espressif/staged-layout/linker/bootstrap/sections.x). |
| ROM | Chip routines available to the images: [S31 ROM symbols](../chips/esp32s31/layout/linker/rom/esp32s31-eco0.x) and [C5 ROM symbols](../chips/esp32c5/layout/linker/rom/esp32c5-eco2.x). |
| RTC retained | Reset-retained records in `.rtc_fast.persistent`, below the reserved tail: [runtime sections](../chips/espressif/staged-layout/linker/runtime/sections.x). |

## Code placement

- The image linker selects strict code by analysis and places its input
  sections in SRAM through `strict-text.x`: [execution in the
  architecture](architecture.md#execution) and [image checks](../tools/image/checks/README.md).
- `.hot.text` places code in SRAM for performance:
  [`place_rx_hot_path!`](../chips/esp32s31/hal/src/ieee80211/dma.rs).
- Assembly, riscv-rt and esp-hal use `.trap`, `.rwtext` and
  `.flash.critical.text` conventions collected by the
  [runtime linker script](../chips/espressif/staged-layout/linker/runtime/sections.x).

## Data placement

- `placed_static!` selects SRAM sections `.critical.data`, `.isr.rodata`
  and `.dma.data`: [placement rustdoc](../chips/espressif/staged-runtime/src/placement.rs).
- `retained_static!` places reset-retained RTC records with a claim in
  zeroed SRAM: [placement rustdoc](../chips/espressif/staged-runtime/src/placement.rs)
  and [retained storage](../portable/memory/src/retained.rs).
- `zeroed_static!` provides an all-zero initializer for statics in regions
  the boot clears: [zeroed storage rustdoc](../portable/memory/src/zeroed.rs)
  in [`oer-memory`](../portable/memory/src/lib.rs).

Memory shared with hardware has its own layout, placement and ownership
contracts: [Hardware memory](hardware-memory.md).

## Stacks

- CPU0's task stack is in PSRAM; each hart has an SRAM interrupt stack
  with a guard region, a debug trigger and stack painting:
  [stacks rustdoc](../chips/espressif/staged-runtime/src/stacks.rs).
- The image linker sizes each hart's IRQ stack to
  `align16(ceil(bound * (100 + margin) / 100)) + guard`. The contract margin
  is 10%; `irq-stack-bytes` is the maximum, used without a proven bound and
  for diagnostic images. Hart 0 on S31 is also rounded to hart 1's guard
  alignment. The second link is final: its bound plus margin must fit the
  allocation minus guard, and its guard and bottom alignment are checked.
  A bound change above 64 bytes fails the build. See the
  [image linker](../tools/image/linker/README.md).
- Task-stack roots, storage and free-space reserves are policy inputs:
  [S31 policy](../chips/esp32s31/platform/stack.toml) and
  [C5 policy](../chips/esp32c5/platform/stack.toml).
- Static bounds and their assumptions are described in [execution in the
  architecture](architecture.md#execution) and the
  [RV32 stack analyzer](../tools/analysis/riscv-stack/README.md).

## Panic

The panic handler records a bounded RTC record without formatting and
resets the chip: [panic rustdoc](../chips/espressif/staged-runtime/src/panic.rs).

## Checks and reports

- `placement`, `stack` and `interrupts` check the image contracts;
  `Product` and `Diagnostic` select interrupt acceptance policies:
  [pipeline contract](../tools/image/build/README.md#contract) and
  [Gates](../tools/image/build/README.md#gates).
- `interrupt-stack.txt`, `interrupt-placement.txt` and
  `strict-placement.toml` record bounds, placement and the link fingerprint:
  [bundle contents](../tools/image/build/README.md#contract).
- Image builds emit compiler frame records with `-Z emit-stack-sizes` and
  take `move-size-limit` from the stack policy:
  [compiler flags in Gates](../tools/image/build/README.md#gates).
