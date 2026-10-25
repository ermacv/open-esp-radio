# ESP32-S31 PAC

The PAC is the exact model of the hardware, generated from the
[register model](../registers/README.md) and its ownership policy by
`cargo registers generate --chip esp32s31`
([renderer](../../../tools/registers/model/README.md#the-thin-pac)). It
holds no logic: every sequence, poll, retry, routing and lifecycle decision
over it is the HAL's, each vendor step with its `SOURCE:`.

| File | What it holds |
| --- | --- |
| [`src/generated.rs`](src/generated.rs) | Generated, never edited: a module per block type with its register modules (masks, offsets, `R`, `W`, `Snapshot`, value types) and the owners of its parts; the field accessors of each field's class; typed DMA pointers; `Interrupt` and `instances` (bases, sizes, bound sources); `ownership` (the partitions, `Claims`, `partition`); the irreducible policy items (whole-word images, masked acknowledgements, interrupt snapshots, whole-word-bits writes); `layouts`; `analog_i2c` |
| [`src/lib.rs`](src/lib.rs) | The root: `take()` claims every part once and returns the partitions |

- **Accessors by class.** Reads, read-modify-writes that keep every bit
  they do not set, writes from zero that take the fields no write may leave
  zero, triggers, doorbells, command starts refusing while busy
  (`oer_mmio::CommandBusy`), write-one-to-clear samples and their
  acknowledgements, FIFO pops and pushes, data-port stores and the strobes
  and stores of shared parts.
- **Typed pointers.** A field the hardware reads as an address is set only
  from an `oer_mmio::DmaRef` of its target layout or a `DmaWindowAddress`,
  never from an integer; a pointer's high field only to its placement's
  prefix.
- **Ownership.** Each part of the policy is one owner type over its words:
  the interrupt and task sides of a block, the parts of a divided block,
  read-only and `Copy` views and shared blocks. A part that serves an
  interrupt names its source (`oer_mmio::InterruptSource`). `take()` is the
  one place the claimed words are assumed exclusive; the generated
  `disjoint_blocks` check proves no two claimed blocks list one word.

`tests/` exercises the generated layouts over host memory; the renderer's
own tests prove the accessors' behaviour and what they refuse.
