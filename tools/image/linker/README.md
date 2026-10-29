# Image linker

`oer-tool-image-linker` checks zeroed input sections, runs LLD, and reuses the
interrupt analysis to select strict SRAM code and each hart's IRQ allocation.
Every staged runtime link needs the fingerprinted `image-linker.toml` supplied
by `cargo fw` or `cargo hil`; ordinary and bootstrap links are unaffected.

## Code-generation preconditions

Every image link checks its actual arguments after recursively expanding
response files, before invoking LLD. `--icf` is refused in every form,
including `--icf=none`; LLD's default remains off. LLVM options passed through
`-mllvm` or `--plugin-opt` cannot enable machine outlining or set
`pick-merged-source-locations`. An explicit LTO pass pipeline cannot
re-enable MergeFunctions or machine outlining. Violations return the typed
`oer_image_checks::codegen::CodegenPrecondition` with the offending option
and its consequence for the stack proof. Build-script link arguments pass
through the same checks.

The [compiler setup](../build/README.md#code-generation-preconditions) pins
MergeFunctions and machine outlining off and validates rustc flags. Together
these conditions preserve the source identity used by the analyzer's
[DWARF Waker-slot rule](../stack/README.md#code-generation-preconditions).

## Placement and stacks

Strict code placement starts from every declared handler on each hart and
level, plus the exception and panic paths. Stack sizing still uses the worst
path of each level; that single path does not limit the code inventory.
Executor task-poll boundaries keep task code separate from strict handler code.
The interrupt float, cached-code and panic-call report uses the full inventory
of those handler contexts, including their task side; exception and panic-entry
paths retain their separate audits.

A complete inventory can place more handler code in SRAM and expose additional
hardware-class or strict-data findings in W0–W3/B0. The selector accounts for
whole input sections and returns an error when they exceed the remaining SRAM;
it never omits a handler to fit. Image budgets and limits are unchanged.

The analysis-only first link uses the contract maximum with zero guards. A proven product bound selects
`align16(ceil(bound * (100 + margin) / 100)) + guard`, with the 10% margin
of the family layer (`chips/espressif/platform/family.toml`). The bound plus margin
must fit the contract maximum without guard bytes. When the full NAPOT guard
does not fit beside it, the allocation keeps the contract maximum and omits the
guard. Guard coverage and unknown reachable frames are reported, never gates.
A hart whose interrupt stack the image policy declares
unused (`[core.N.interrupt_stack] unused = true`)
gets 0 bytes in both links and takes no part in the sizing. Partial bounds,
diagnostic and probe images retain the maximum for each used hart. Existing
image-policy maximum overrides remain effective, including
diagnostic-lower-mac's 65536 bytes.

One second link supplies `IRQ_STACK_HART<N>_BYTES` for each used hart
alongside `strict-text.x`. Analysis of that final ELF must fit the bound plus
the rounded-up margin inside its actual allocation minus guard. A lost proof,
a bound change above 64 bytes or insufficient capacity fails with figures; there is no third link or fallback to the maximum.
The final ELF carries the actual guard size, including zero; the runtime arms
only a present guard. Linker assertions and checks enforce the NAPOT alignment
of each present guard. `Interrupt guards` reports negative spare and unknown frames
without failing build, run or analysis.
The first MTVT table requires 256-byte alignment, so `.isr.text` starts on
that boundary; the gap after `.irq_stacks` is at most 255 bytes and is not
included in either section.

An image that declares `task-stack = "proven"` also gets each used hart's task stack
sized by the same second link: `align16(bound + minimum_free_bytes)` of the
policy's `core.N.task_stack`, passed as `TASK_STACK_HART<N>_BYTES`. Each bottom
is aligned to 64 bytes within `.psram.task_stacks` and carries the runtime's
stack watchpoint, so the size has no other constraint; the 16 bytes keep the
stack pointer aligned. The first-pass bound must be complete and rest only on
admitted assumptions. After the second link the bound must differ by at most
64 bytes and bound plus reserve must fit the allocation; otherwise the link
fails with figures and attempts nothing further. A policy that also sets
`allocation_bytes` names the size twice and is an error. The image's
placement range for the stack is then exactly the linked size.
Images without the key keep `allocation_bytes`, else the chip stack policy's
`default_task_stack_bytes` entry for that hart.

`strict-placement.toml` retains first-pass bounds, selected sizes and their
sources, together with the final ELF/input fingerprint. `interrupt-stack.txt`
reports final bounds, margins, guards, allocations and size sources per hart.
The checks read actual storage from the ELF; `irq-stack-bytes` and placement
`maximum-bytes` are ceilings. The chip contract's percentage supplies the IRQ margin. W1 reports its
watermark and `cargo fw run w1` holds its observed depth to the final ELF's
proven bound, as HIL does, instead of requiring 4096 free bytes.

`oer-staged-runtime-stacks.x` is generated by the chip layout build from the
fact geometry. It contains all N IRQ/task ranges and their assertions; a
zero allocation reserves no storage. The runtime's generated address tables
use that same geometry. `strict-placement.toml` keys task selections by hart,
and the final pass verifies the complete inventory and each bound separately.
Regenerate exact vector coverage with
`cargo run -p oer-chip-profile --example stack-coverage`.
