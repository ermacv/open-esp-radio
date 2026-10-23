# Image linker

`oer-image-linker` checks zeroed input sections, runs LLD, and reuses the
interrupt analysis to select strict SRAM code and each hart's IRQ allocation.
Every staged runtime link needs the fingerprinted `image-linker.toml` supplied
by `cargo fw` or `cargo hil`; ordinary and bootstrap links are unaffected.

The first link uses the contract maximum. A proven product bound selects
`align16(ceil(bound * (100 + margin) / 100)) + guard`, with the 10% margin
from `chip.toml`. On S31, hart 0 is additionally rounded to hart 1's guard
alignment, because its top is hart 1's bottom. Partial bounds, diagnostic
and probe images retain the maximum. Existing image-policy maximum overrides
remain effective, including diagnostic-lower-mac's 65536 bytes per hart.

One second link supplies `IRQ_STACK_HART0_BYTES` and `IRQ_STACK_HART1_BYTES`
alongside `strict-text.x`. Analysis of that final ELF must fit the bound plus
the rounded-up margin inside its actual allocation minus guard. A lost proof,
a bound change above 64 bytes, insufficient capacity, or a guard violation
fails with figures; there is no third link or fallback to the maximum.
Linker assertions and the checks enforce the NAPOT alignment of each bottom.
The first MTVT table requires 256-byte alignment, so `.isr.text` starts on
that boundary; the gap after `.irq_stacks` is at most 255 bytes and is not
included in either section.

`strict-placement.toml` retains first-pass bounds, selected sizes and their
sources, together with the final ELF/input fingerprint. `interrupt-stack.txt`
reports final bounds, margins, guards, allocations and size sources per hart.
The checks read actual storage from the ELF; `irq-stack-bytes` and placement
`maximum-bytes` are ceilings. The IRQ `minimum_free_bytes` policy field was
removed: the chip contract's percentage supplies the margin. W1 reports its
watermark and `cargo fw run w1` holds its observed depth to the final ELF's
proven bound, as HIL does, instead of requiring 4096 free bytes.
