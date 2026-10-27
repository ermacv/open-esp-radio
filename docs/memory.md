# Memory and stacks

Entry points for the staged images' memory layout, placement and stack
contracts; details live with their owners.

## Regions

The [shared layout](../chips/espressif/staged-layout/README.md) defines the
regions; their addresses and sizes are the
[chip facts](../chips/espressif/facts/README.md): the chip layer's internal
SRAM, flash window and retained memory with each region's attributes, the
module's PSRAM, `runtime-psram` after the family's bootstrap probe page, what
the boot chain keeps and what the ROM of the chip's revision uses of the
internal SRAM. The application's SRAM is the internal SRAM below the boot
chain's reserve, clear of the ROM's data. The
[S31](../chips/esp32s31/layout/src/lib.rs) and
[C5](../chips/esp32c5/layout/src/lib.rs) layout crates generate their
`LAYOUT`, and through it the linker scripts' `--defsym` values, from those
facts at build time; the host checks read the same facts.

| Region | Current use and source |
| --- | --- |
| SRAM | The complete bootstrap code and ordinary constants; runtime strict and hot code, interrupt stacks, critical state and DMA storage: [runtime sections](../chips/espressif/staged-layout/linker/runtime/sections.x). |
| PSRAM | Ordinary runtime code, data and the per-hart task stacks: [runtime sections](../chips/espressif/staged-layout/linker/runtime/sections.x). |
| Flash XIP | Explicit bootstrap metadata, stage-two payload and tuning-reference data: [bootstrap sections](../chips/espressif/staged-layout/linker/bootstrap/sections.x). |
| ROM | Chip routines available to the images: [S31 ROM facts](../chips/esp32s31/platform/rom-rev0.toml) generate its ROM script in the layout build directory; [C5 ROM symbols](../chips/esp32c5/layout/linker/rom/esp32c5-eco2.x). |
| RTC retained | Reset-retained records in `.rtc_fast.persistent`, below the reserved tail: [runtime sections](../chips/espressif/staged-layout/linker/runtime/sections.x). |

## Code placement

- The image linker selects strict code by analysis and places its input
  sections in SRAM through `strict-text.x`: [execution in the
  architecture](architecture.md#execution) and [image checks](../tools/image/checks/README.md).
- `.hot.text` places code in SRAM for performance:
  [`place_rx_hot_path!`](../chips/esp32s31/hal/src/ieee80211/dma.rs).
- The staged platform uses `.trap`, `.rwtext` and
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
- Executor running-context storage has one slot for each facts-defined hart.
  Executor identities remain global software-source identities; their packed
  assignment/poll words derive hart-bit width and the unassigned sentinel
  from `HARTS`, retaining the timestamp wrap bound in const checks.
  Each executor publishes its composition's immutable urgent-source list before
  polling; the source type comes from its doorbell constructor, through a
  copyable zero-sized observation. Its list reference stays in zeroed critical
  SRAM. The runtime's
  zero-sized hart token survives interrupt enable while stack-install authority
  is consumed. Current images retain every task and waker on the boot hart.

Memory shared with hardware has its own layout, placement and ownership
contracts: [Hardware memory](hardware-memory.md).

## Stacks

- **Every image accounts for every hart** in the chip facts' `[interrupts] harts`.
  Each hart has a task stack and an interrupt stack. The policy
  (`[core.N.task_stack]`, `[core.N.interrupt_stack]`) declares both used
  or both `unused = true`. Unused ranges link zero
  bytes; the image gate rejects linked starters, routes or storage that
  would use them. Every current S31 image runs on the boot hart and keeps
  all secondary harts contained: the HAL stalls them, gates their clocks,
  holds their resets and clears their ROM boot addresses
  ([secondary boot](../chips/esp32s31/notes/core1-boot.md)). Staged entries
  admit only the boot hart. The runtime's critical section masks only its
  executing hart and takes no cross-hart lock
  ([critical-section rustdoc](../chips/espressif/staged-runtime/src/critical.rs)).
  The critical section is hart-local; the image check confines it to one declared
  hart through `[package.metadata.open-radio] critical-section-hart`.
- `oer-staged-runtime-stacks.x` is generated from the same hart geometry as
  the vectors. It places each task stack in PSRAM and each interrupt stack
  in SRAM, with exact-size, memory-range, alignment and guard assertions.
  Runtime linker-marker tables are generated and indexed by hart. BSS
  follows all task ranges; empty ranges contribute no allocation.
- An image that declares `task-stack = "proven"` sizes every used task
  stack to `align16(bound + minimum_free_bytes)` in the final link. Other
  images use that hart's policy `allocation_bytes`, else its
  `default_task_stack_bytes` policy entry or chip layout default. Without
  the declaration the boot hart's task bound may be partial: its known part
  must fit and unresolved sites are reported. Secondary task roots require
  complete bounds.
  Defaults cover the entire geometry, including explicit zeroes for contained
  harts. See the [image linker](../tools/image/linker/README.md).
- The image linker sizes each used hart's IRQ stack to
  `align16(ceil(bound * (100 + margin) / 100)) + guard`. The contract margin
  is 10%; `irq-stack-bytes` is the maximum, used without a proven bound and
  for diagnostic images. The second link is final: its bound plus margin must fit the
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
