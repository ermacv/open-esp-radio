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
contract. The image linker runs the same interrupt analysis and `CodePlacement`
after its first link. It resolves strict cached functions to their defining
input sections using the objects' symbol tables, selects those whole sections
in the generated `strict-text.x` inside `.isr.text`, and links once more.
With no strict cached functions the fragment stays empty and the first ELF is
final. Strict code has no manual function-section annotations. Assembly entry,
riscv-rt and esp-hal section conventions still place their code in SRAM;
`.hot.text` selects code for performance. The post-build interrupt check
verifies the final ELF.

Analysis runs if and only if the link uses the staged runtime script
(`-Truntime/link.x`, supplied by `configure_runtime`). Such a link requires
`OER_IMAGE_INTERRUPTS`, the path to `image-linker.toml` in its output directory.
Without it the linker fails before invoking LLD, naming the bin and directing
the caller to `cargo fw build`, `cargo fw probe` or `cargo hil`.
A link using another script ignores the specification even when it is present:
the other bin of a probe package and the bootstrap link normally, without
analysis or generated placement files.

The specification names the repository, chip contract and stack policy;
the analysis loads that chip's pinned ROM and summaries exactly as the check does.
The linker compares whole selected sections with the same free-SRAM measurement
before its second invocation. An analysis error, an unrepresentable script name
or insufficient SRAM fails the link. A diagnostic's partial stack still supplies
its known strict functions. Bootstrap has no analysis specification.

`interrupt-stack.txt` reports each hart's guard, exception entry and path,
largest reachable own frame and remaining guard bytes, as well as strict code in cached memory by hart/level,
P2 task code in PSRAM and its task roots, static cached data references and
sized objects, and usable/placed/free SRAM. `interrupt-placement.txt`, listed
in the bundle manifest, gives the full cached function inventory with
`strict`/`task` classification, crate totals, strict SRAM/ROM/cache totals,
paths to strict cached functions, data references and objects, and SRAM
ranges. Code sizes come from the existing analysis. SRAM usage counts the
union of allocated sections and placement ranges, including overlapping
NOLOAD stacks and DMA; the usable contract already excludes the ROM tail.
Data without sized symbols remains visible without an invented size.
Both reports include `strict code placed by the image linker: N functions, B bytes`.
The placement inventory also lists each selected function's actual input section,
final size and the other symbols moved with that section. `strict-placement.toml`
and the generated fragment remain in the bundle.

The existing checks of floating-point instructions, interrupt entry points,
direct handler calls, nesting rules, bounds and admitted assumptions still
apply to the full resolved interrupt context, including its tasks.

Run host regressions with `cargo test -p oer-image-checks`.
