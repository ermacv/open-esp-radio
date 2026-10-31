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
[S31 layout crate](../chips/esp32s31/layout/src/lib.rs) generates its
`LAYOUT`, and through it the linker scripts' `--defsym` values, from those
facts at build time; the host checks read the same facts.

| Region | Current use and source |
| --- | --- |
| SRAM | The complete bootstrap code and ordinary constants; runtime strict and hot code, interrupt stacks, critical state and DMA storage: [runtime sections](../chips/espressif/staged-layout/linker/runtime/sections.x). |
| PSRAM | Ordinary runtime code, data and the per-hart task stacks: [runtime sections](../chips/espressif/staged-layout/linker/runtime/sections.x). |
| Flash XIP | Explicit bootstrap metadata, stage-two payload and tuning-reference data: [bootstrap sections](../chips/espressif/staged-layout/linker/bootstrap/sections.x). |
| ROM | Chip routines available to the images: [S31 ROM facts](../chips/esp32s31/platform/rom-rev0.toml) generate its ROM script in the layout build directory. |
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
  SRAM through `zeroed_static!`; `OnceValue` derives its unpublished zero
  representation independently of its payload. The runtime's
  zero-sized hart token survives interrupt enable while stack-install authority
  is consumed. Each executor's tasks and wakers stay on its own hart;
  boot-smoke runs an interrupt-mode executor on hart 1. Statics that two
  harts share (a cross-hart signal's facts word, the in-place request slot,
  a secondary hart's launch mailbox) sit in critical SRAM
  (`placed_static!`, `secondary_entry!`).

Memory shared with hardware has its own layout, placement and ownership
contracts: [Hardware memory](hardware-memory.md).

## Stacks

- **Every image accounts for every hart** in the chip facts' `[interrupts] harts`.
  Each hart has a task stack and an interrupt stack. The policy
  (`[core.N.task_stack]`, `[core.N.interrupt_stack]`) declares both used
  or both `unused = true`, or, for a secondary hart, both a launch template
  (`launch = "declared"`): the hart runs only in an image that declares it
  in `[package.metadata.open-radio] harts`, and is unused in every other.
  Unused ranges link zero bytes; the image gate rejects linked starters,
  routes or storage that would use them. The bootstrap contains every
  secondary hart: it stalls them, gates their clocks, holds their resets and
  clears their ROM boot addresses
  ([secondary boot](../chips/esp32s31/platform/README.md#secondary-hart-launch-and-containment)),
  and opens a declared hart's instruction bus after it published the
  runtime's code. A declared hart starts during boot, before any executor,
  through its generated entry, which installs its own PSRAM task stack and
  calls the image's root for the hart (`secondary_entry!`); the boot hart's
  staged entries admit only the boot hart. A secondary hart's task root,
  interrupt handlers and exception root have complete stack bounds, and its
  interrupt stack is sized by the image linker like the boot hart's. The runtime's critical section masks only its
  executing hart and takes no cross-hart lock
  ([critical-section rustdoc](../chips/espressif/staged-runtime/src/critical.rs)).
  The staged build/run check confines linked critical-section entry points to
  the hart roots declared through `[package.metadata.open-radio] critical-section-hart`;
  a secondary hart's roots must not reach it.
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
  harts; a declared launch template takes its `launch_bytes` before a proven
  bound sizes it, and its interrupt stack the contract maximum. The
  stage-two header's used-hart word names every hart with a task stack, and
  the image gate holds it to the policy. See the [image linker](../tools/image/linker/README.md).
- The staged runtime exports `_runtime_exception_frames` from its trap policy.
  The IRQ bound adds that many complete exception entry/handler bounds above
  the per-level interrupt sum. Its reset gate permits one platform reset attempt;
  with intact SRAM/entry/local mask code, capture, a stop/hook fault and a reset
  fault need at most three copies. Stack allocation and the existing margin
  include those copies; a fault before a complete frame remains outside the
  software guarantee. The FPU remains disabled throughout the trap closure.
- The image linker sizes each used hart's IRQ stack to
  `align16(ceil(bound * (100 + margin) / 100)) + guard`. The contract margin
  is 10%; `irq-stack-bytes` is the maximum, used without a proven bound and
  for diagnostic images. The second link is final: its bound plus margin must fit the
  allocation minus any placed guard. The bound plus margin gates against the
  maximum without guard bytes; a full guard that does not fit beside it is
  omitted and reported. Guard frame coverage, unknown frames and guard storage
  are reported, never gates. Present guards retain their NAPOT bottom alignment.
  A bound change above 64 bytes fails the build. See the
  [image linker](../tools/image/linker/README.md).
- Task-stack roots, storage and free-space reserves are policy inputs:
  [S31 policy](../chips/esp32s31/platform/stack.toml).
- Static bounds and their assumptions are described in [execution in the
  architecture](architecture.md#execution) and the
  [RV32 stack analyzer](../tools/image/stack/README.md).

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
  [bundle contents](../tools/image/bundle/README.md).
- Image builds emit compiler frame records with `-Z emit-stack-sizes` and
  take `move-size-limit` from the stack policy as a warning threshold;
  frame sizes and image panic budgets are reports, while proven stack bounds
  and placement remain gates:
  [compiler flags in Gates](../tools/image/build/README.md#gates).
