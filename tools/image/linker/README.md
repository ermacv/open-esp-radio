# Image linker

`oer-image-linker` checks zeroed input sections, runs LLD, and reuses the
interrupt analysis to select strict SRAM code and each hart's IRQ allocation.
Every staged runtime link needs the fingerprinted `image-linker.toml` supplied
by `cargo fw` or `cargo hil`; ordinary and bootstrap links are unaffected.

The first link uses the contract maximum. A proven product bound selects
`align16(ceil(bound * (100 + margin) / 100)) + guard`, with the 10% margin
of the family layer (`chips/espressif/platform/family.toml`). A hart whose interrupt stack the image policy declares
unused (`[core.N.interrupt_stack] unused = true`, S31 hart 1 in every image)
gets 0 bytes in both links and takes no part in the sizing. Partial bounds,
diagnostic and probe images retain the maximum for each used hart. Existing
image-policy maximum overrides remain effective, including
diagnostic-lower-mac's 65536 bytes.

One second link supplies `IRQ_STACK_HART0_BYTES` (and `IRQ_STACK_HART1_BYTES`
for a used hart 1) alongside `strict-text.x`. Analysis of that final ELF must fit the bound plus
the rounded-up margin inside its actual allocation minus guard. A lost proof,
a bound change above 64 bytes, insufficient capacity, or a guard violation
fails with figures; there is no third link or fallback to the maximum.
Linker assertions and the checks enforce the NAPOT alignment of each bottom.
The first MTVT table requires 256-byte alignment, so `.isr.text` starts on
that boundary; the gap after `.irq_stacks` is at most 255 bytes and is not
included in either section.

An image that declares `task-stack = "proven"` also gets its CPU0 task stack
sized by the same second link: `align16(bound + minimum_free_bytes)` of the
policy's `core.0.task_stack`, passed as `CPU0_PSRAM_TASK_STACK_BYTES`. The bottom
is the 64-byte aligned start of `.psram.task_stacks` and carries the runtime's
stack watchpoint, so the size has no other constraint; the 16 bytes keep the
stack pointer aligned. The first-pass bound must be complete and rest only on
admitted assumptions. After the second link the bound must differ by at most
64 bytes and bound plus reserve must fit the allocation; otherwise the link
fails with figures and attempts nothing further. A policy that also sets
`allocation_bytes` names the size twice and is an error. The image's
placement range for the stack is then exactly the linked size.
Images without the key keep `allocation_bytes`, else the chip stack policy's
`default_cpu0_task_stack_bytes`.

`strict-placement.toml` retains first-pass bounds, selected sizes and their
sources, together with the final ELF/input fingerprint. `interrupt-stack.txt`
reports final bounds, margins, guards, allocations and size sources per hart.
The checks read actual storage from the ELF; `irq-stack-bytes` and placement
`maximum-bytes` are ceilings. The IRQ `minimum_free_bytes` policy field was
removed: the chip contract's percentage supplies the margin. W1 reports its
watermark and `cargo fw run w1` holds its observed depth to the final ELF's
proven bound, as HIL does, instead of requiring 4096 free bytes.
