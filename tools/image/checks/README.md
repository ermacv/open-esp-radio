# Image checks

`oer-image-checks` owns the image pipeline's interrupt, stack, placement and
post-link input checks. The chip profile and stack policy supply their
contracts; `cargo fw build`, including analysis builds, requires the
`interrupts` check to pass.

Interrupt entries, handlers and the executor code they run must execute from
SRAM or the analysed mask ROM. The panic path, compiler panic entries and the
image panic hook retain their independent checks; static references from the
hook and its resolved callees must also avoid cached memory.

The gate separates P2 task polls using resolution facts. For each hart and
priority it follows the bound's root with exactly that context's resolutions.
`Analysis::reached_cut` stops at the dequeue calls identified by `TaskPolls` or
`NoTasks`, retaining their owning functions. Functions beyond the cut are
tasks; any function also reached on a strict path remains strict. Only P2
grants this task-side permission: its tasks and callees may execute from
PSRAM as well as SRAM/ROM. Cached flash XIP is forbidden on both sides.
Other priorities and exceptions retain strict placement throughout.

The S31 accesses flash and PSRAM through separate MSPI controllers: a flash
operation does not disable the PSRAM cache. Placement follows this hardware
contract. Our strict helpers use `#[esp_hal::ram]`; exact input-section rules
in the staged runtime's `.isr.text` place the required pinned dependency
functions in SRAM.

`interrupt-stack.txt` reports strict code in cached memory by hart/level,
P2 task code in PSRAM and its task roots, static cached data references and
sized objects, and usable/placed/free SRAM. `interrupt-placement.txt`, listed
in the bundle manifest, gives the full cached function inventory with
`strict`/`task` classification, crate totals, strict SRAM/ROM/cache totals,
paths to strict cached functions, data references and objects, and SRAM
ranges. Code sizes come from the existing analysis. SRAM usage counts the
union of allocated sections and placement ranges, including overlapping
NOLOAD stacks and DMA; the usable contract already excludes the ROM tail.
Data without sized symbols remains visible without an invented size.

The existing checks of floating-point instructions, interrupt entry points,
direct handler calls, nesting rules, bounds and admitted assumptions still
apply to the full resolved interrupt context, including its tasks.

Run host regressions with `cargo test -p oer-image-checks`.
