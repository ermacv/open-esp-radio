# Image linker

`oer-tools-image-linker` checks zeroed input sections, runs LLD, and reuses the
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
`oer_tools_image_checks::codegen::CodegenPrecondition` with the offending option
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
hardware-class or strict-data findings in staged images. The selector accounts for
whole input sections and returns an error when they exceed the remaining SRAM;
it never omits a handler to fit. Image budgets and limits are unchanged.

The analysis-only first link places every used interrupt stack at the stack
policy's `analysis_irq_stack_bytes` with zero guards: a placeholder to link
and analyse with, no limit. The second link sizes each used hart's stack:
a proven bound takes `align16(ceil(bound * (100 + margin) / 100)) + guard`,
with the image's margin (`[package.metadata.open-radio]
irq-stack-margin-percent`, zero included) or else the policy's
`irq_stack_margin_percent`; a hart whose bound is not proven takes the size
its image declares (`irq-stack = { boot = <bytes>, 1 = <bytes> }`, harts
named as `harts` names them) plus its guard, with no margin added. A size
has one source: a hart with neither a proof nor a declaration fails the
build naming the key and the hart, and a declaration for a hart whose bound
is proven fails naming the proof. The diagnostic flag of a probe build
changes neither. The guards are the stack policy's (`irq_stack_guard_bytes`),
placed unless the image declines them (`irq-stack-guard = "declined"`:
zero guard bytes for every hart). Hart 0's stack is rounded to hart 1's
placed guard so that the guard is aligned; what limits a stack is the SRAM, and stacks that do
not fit fail with their figures. The SRAM left for strict code is counted
from the selected sizes, so the placeholder does not change the result.
Guard coverage and unknown reachable frames are reported, never gates.
A hart whose interrupt stack the image policy declares
unused (`[core.N.interrupt_stack] unused = true`), or a launch template the
image does not declare in its `harts`, gets 0 bytes in both links and takes
no part in the sizing.

Before the first link it writes `hot-text.x` beside the specification from
the image's `hot` level (`oer_tools_image_checks::linker::hot`): an output
section `.hot.<N>.text` in SRAM a level the image takes, the same in both
links ([code placement](../../../docs/memory.md#code-placement)).

One second link supplies `IRQ_STACK_HART<N>_BYTES` for each used hart
alongside `strict-text.x`. The final ELF's bound must equal the first
link's: nothing the second link does changes a frame or an edge, so a
difference is a fault of the analysis and fails with both bounds. A proven
bound with its margin must fit the actual allocation minus guard. There is no
third link.
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
admitted assumptions. After the second link the bound must equal the first
link's, as an interrupt stack's must (the second link changes only the
stacks' sizes and symbols), and bound plus reserve must fit the allocation;
otherwise the link fails with both bounds and attempts nothing further. The
reserve is the policy's `minimum_free_bytes` unless the image sets
`task-stack-reserve-bytes` (zero included) beside `task-stack = "proven"`;
the key without the proven bound fails the build, since under a partial
bound the reserve covers the unresolved sites. A policy that also sets
`allocation_bytes` names the size twice and is an error. The image's
placement range for the stack is then exactly the linked size.
A declared launch template links its `launch_bytes` in the first link. A
proven frameless root links exactly its reserve. Images without the key keep
`allocation_bytes`, else the chip stack policy's `default_task_stack_bytes`
entry for that hart, else a declared template's `launch_bytes`.

`strict-placement.toml` retains first-pass bounds, selected sizes and their
sources, together with the final ELF/input fingerprint. `interrupt-stack.txt`
reports final bounds, margins, guards, allocations and size sources per hart.
The checks read actual storage from the ELF. HIL compares observed
watermarks with the final ELF's proven bounds.

`oer-staged-runtime-stacks.x` is generated by the chip layout build from the
fact geometry. It contains all N IRQ/task ranges and their assertions; a
zero allocation reserves no storage. The runtime's generated address tables
use that same geometry. `strict-placement.toml` keys task selections by hart,
and the final pass verifies the complete inventory and each bound separately.
Regenerate exact vector coverage with
`cargo run -p oer-tools-chip-profile --example stack-coverage`.
