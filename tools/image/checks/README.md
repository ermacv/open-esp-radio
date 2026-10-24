# Image checks

`oer-image-checks` owns the image pipeline's frame, interrupt, stack, placement and
post-link input checks. The chip profile and stack policy supply their
contracts; `cargo fw build`, including analysis builds, requires the
`frames` and `interrupts` checks to pass.

The `frames` check bounds every compiler frame record in both the runtime
and bootstrap ELF by the stack policy's `max_move_bytes` (currently 4096).
It uses the existing `Image::stack_sizes`, including functions no root reaches.
It does not invent sizes for missing records: the stack coverage check owns
those. `Required::Product` rejects an unexcepted large frame;
`Required::Diagnostic` reports it as a warning, as the IRQ guard check does.
The runtime shares its analysis with the task and interrupt checks; bootstrap
shares its analysis between the frame and bootstrap-stack checks.

Both runtime and bootstrap outcomes export the ROM summaries their analyses
applied and the frame coverage reviews (`stack-coverage.toml` symbols) their
stack checks applied. The image bundle records each union, so the catalog's
stale-summary audit includes summaries used only during bootstrap, and
`cargo hil images check --all` without `--chip` fails on a coverage review
no image of the chips sharing its file applied.

The stack and interrupt checks also fail when a root's walk, which follows
calls into the ROM, reaches a function overlapping a `[rom].forbidden` range
of `chip.toml`, naming the path that reached it.

The stack check also proves each core of the chip: `cores.txt` lists every
core's task stack and interrupt stack, used with its linked bytes or `unused`
(0 bytes). An unused stack fails the build when the image links a core starter
(`[core-start]` of `chip.toml`), links storage for it, or routes an interrupt
to its core; a used secondary core runs only with a proven bound. The
bootstrap links no starter.

An image's `stack.toml` (schema 6) may name exact exceptions:

```toml
[[frame_exceptions.runtime]] # or frame_exceptions.bootstrap
function = "example::receive::<example::Packet, 1664>"
reason = "The owned receive packet and decode result occupy this frame."
ceiling_bytes = 8192
```

`function` is the complete demangled symbol without Rust hashes, as printed
by `oer_analysis_elf::demangle` and the frame report. Concrete type and const
arguments, closure identities and compiler shims remain part of the key;
there are no prefixes or wildcards. Matching examines every alias; aliases
at one address count as one implementation. A key matching zero or multiple
implementations fails. Duplicate exceptions, including different aliases of
one implementation, also fail.

The initial ceiling is the measured frame plus 64 bytes, the growth a change
may report without stopping. Subsequent policy changes may only decrease it;
review must reject increases. A frame exceeding its ceiling
fails, even for Diagnostic. An exception whose frame now fits the move limit
is stale and fails: remove the entry. An exception cannot supply a missing
compiler record. These policy errors fail under either severity.

An extending policy can append exceptions while retaining its base's stack
allocations, reserves and move limit. W1 has its own `stack.toml`; configured
W3 uses `stack-configured.toml`, extending W3's `stack.toml`. Unconfigured W3
does not link these implementations, so it uses the base without exceptions.
The same configured policy serves `--analysis` and stand-configured W3.

`runtime-frames.txt` and `bootstrap-frames.txt` report the limit, recorded
functions and count above the limit, each large function, and each applied
exception's current frame, ceiling, spare bytes and reason. Diagnostic
warnings and failed checks are written into these reports before returning
the verdict. Frame exceptions do not relax IRQ guards or whole-stack bounds.

Interrupt entries, handlers and the executor code they run must execute from
SRAM or the analysed mask ROM. The panic path, compiler panic entries and the
image panic hook retain their independent checks; static references from the
hook and its resolved callees must also avoid cached memory.

Each interrupt table entry declares its class (`oer_interrupt_table::Class`,
the byte at `table-class`); the level of a hart's `Executor` entry is the
executor's level, every other level is a hardware level. The class, never a
level number or a handler's name, decides the rules:

- **Placement.** Two executors on one level and core, a `Hardware` or
  `Exempt(WakerCall)` entry on its core's executor level, or an
  `Exempt(ExecutorLevel)` entry with no executor beside it fail every policy
  (`interrupt_table!` refuses them at compile time).
- **Hardware** levels, and the handler of each `Exempt` entry, run strict
  code throughout: from SRAM/ROM, with no task poll (the executor's direct
  calls of its recorded task poll boundaries) and no call through a waker's vtable (`WakerVtables` sites).
  `Required::Product` fails a breach, `Required::Diagnostic` warns per
  context. The cached data their strict code references is reported per
  context, writable (`.data`/`.bss`) and read-only objects by name, and not
  gated yet.
- **Executor** levels separate their task polls. For each hart and priority
  the gate follows the bound's root with exactly that context's resolutions.
  `Analysis::reached_cut` stops at the executor's direct calls of its
  recorded task poll boundaries (`task_poll_sites`), retaining their owning
  functions.
  Functions beyond the cut are tasks; any function also reached on a strict
  path remains strict. Only an executor's level grants this task-side
  permission: its tasks and callees may execute from PSRAM as well as
  SRAM/ROM. Cached flash XIP is forbidden on both sides. The strict part's
  cached data is reported as for hardware levels; its waker calls are not
  checked (an executor wakes its own tasks).
- **Exempt** entries are hardware handlers that break one named rule:
  `ExecutorLevel` (on their core's executor level) or `WakerCall` (a waker
  call that only their own handler reaches is waived; the same site reached
  from the dispatcher or another entry still fails). `interrupt-stack.txt`
  prints each one's violation and reason, the bytes of the symbol
  `table-exemption-prefix` plus its handler symbol, and lists the waived
  sites.

Exceptions retain strict placement throughout. Calls into the panic machinery
from each context's strict code are reported with their sites, not gated.

The `panics` check counts the panic sites each root reaches: a direct call or
jump into a panic entry (`core::panicking::*`, `unwrap_failed`,
`expect_failed`, the slice-index failures, `panic_already_*`,
`rust_begin_unwind`, `oer_platform_panic_hook`) from a function that is not
one itself. The roots are each interrupt context (hart and level, each
hart's exception), CPU0 (stage two's entry) and each task root in the
runtime, and the bootstrap root; it runs with the stack and interrupts
checks, over their bounds' `reached` inventories. A site belongs to the
layer of the crate its function's demangled name starts with (the self type
of `<T as Trait>`): workspace crates by directory (`chips/**/pac`,
`pac-raw`, `*-pac`, `*-pac-raw` are `pac`; `chips/*/hal` is `hal`; the rest
of `chips/` is `chips`; `portable`, `protocols`, `firmware`), then `core`
(core, alloc, compiler_builtins), `embassy` (`embassy_*`), `esp-hal`
(`esp_*`, `esp32*`), `third-party` (every other crate) and `unnamed` (names
without a crate: `#[no_mangle]` and assembly symbols). An inlined call counts in the function it is inlined into.

The image's `[package.metadata.open-radio.panic-budget]` gives each ELF's
budget by layer, sites counted once over all its roots:

```toml
[package.metadata.open-radio.panic-budget]
runtime = { chips = 40, core = 12, hardware = 6 }
bootstrap = { core = 3 }
```

`hardware` budgets the distinct sites all `Hardware` interrupt contexts
reach together, whatever their layer. A key it does not name has 0, the
target of every budget. A count above its budget fails; a count below it is
a warning naming the lower value. A budget only shrinks: review rejects an increase. Variants of
one package (`--analysis`, HIL classes) share its budget, the maximum of
their counts. `runtime-panics.txt` and `bootstrap-panics.txt` give the
counts and budgets by layer and for `hardware`, each root's sites by layer, and every site with
its caller, entry and the roots that reach it.

The S31 accesses flash and PSRAM through separate MSPI controllers: a flash
operation does not disable the PSRAM cache. Placement follows this hardware
contract. The image linker runs the same interrupt analysis and `CodePlacement`
after its first link. It resolves strict cached functions to their defining
input sections using the objects' symbol tables, selects those whole sections
in the generated `strict-text.x` inside `.isr.text`, and links once more.
The same second link sizes the IRQ stacks independently by hart; see the
[image linker](../linker/README.md). Even with an empty strict fragment, a
proven product bound selects its bound plus the contract's 10% margin, rounded
up to 16 bytes, plus its guard; an unproven or diagnostic stack keeps the
contract maximum. Checks use the actual linked ranges, and placement
`maximum-bytes` limits their capacity. A second-pass bound change above 64
bytes or a bound plus margin that no longer fits fails without another link.
Strict code has no manual function-section annotations. Assembly entry,
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

`interrupt-stack.txt` reports each hart's bound, rounded-up percentage margin,
guard, allocated bytes and size source (`proven bound`, `conditional bound
(<assumptions>)` for a bound on admitted assumptions or the nesting rule, or
`contract maximum`),
as well as its exception entry and path,
largest reachable own frame and remaining guard bytes, as well as strict code in cached memory by hart/level,
each table entry's class, exemption reasons, hardware-class breaches,
panic sites of strict code by context, executor task code in PSRAM and its
task roots, static cached data references and
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

## Size summary format

`sizes.json` schema 2 is published by `oer-image-build` beside the image's
ELFs. The check outcome carries `sizes::Measurements` from the same `Image`
used for checking. During a normal build, task bounds and context-specific
interrupt bounds are captured when the checks compute them; no second analysis
or firmware compilation is run. The explicit refresh command reanalyses saved
ELFs without compiling or changing the original reports.

The top level contains `schema`, `type_min_bytes` (the constant
`oer_image_checks::sizes::TYPE_MIN_BYTES`, **256 B**), `provenance`, `elfs`
(`runtime` and `bootstrap`), `image_files` and `embedded_payloads`. The latter
records the byte-verified runtime copy excluded from the image code total. Provenance contains commit/dirty status, rustc's
verbose version, chip, target, profile/settings, application/features, purpose,
layout seed, configuration digests, compiler flags and stack policy source
paths/digests. Values that may contain stand credentials are hashed.

Each ELF contains:

- `sections`: every `SHF_ALLOC` section, keyed by name, with memory kind from
  the chip map, content class, input-free filler ranges, `executable`, `writable`,
  `nobits`, address and byte size. `totals` gives executable/read-only contents,
  filler and owned RAM separately per memory kind (`sram`, `psram`, `flash`;
  a ROM companion allocates no image section). Content totals exclude filler;
  image-level accounting also subtracts the embedded payload copy. Overlapping
  NOLOAD RAM sections remain separately visible, so RAM totals are not a
  physical free-memory estimate.
- `bootstrap-stack.txt` names the end of the bootstrap stack's path when the
  policy's `[bootstrap_stack] handoff` (the stage-two entry's symbol) holds:
  the bootstrap jumps to the constant entry the build knows (`hand_off::<ENTRY>`,
  which halts if the payload header names another), the checks find that
  jump in the bootstrap ELF, and a scan of the stage-two ELF shows every path
  from the entry installing the CPU0 task stack's top as `sp` before anything
  reads it. Otherwise the jump stays a hole.
- `runtime-stack.txt` and `bootstrap-stack.txt` label each bound `proven`
  only when it rests on no assumption, `conditional (<assumptions>)`, such as
  `conditional (ExecutorInvariant)`, when it does, and `partial` without a
  number; `interrupt-stack.txt` labels each hart the same way, with `nesting`
  where a handler may lower the interrupt level. `runtime-stack.txt` ends with
  each task stack's allocation and where it came from: the bound (labelled
  likewise) plus the policy's reserve, the policy's `allocation_bytes`, or
  the chip layout's default.
- `runtime-indirect-sites.txt`/`.json` and `bootstrap-indirect-sites.txt`/`.json`
  list, for each root of the reachability reports (each hart/priority and its
  exception, CPU0 and the task roots; the bootstrap root), every indirect site
  its bound took (`Bound::sites`): the site, its function and offset, the
  number of targets, the facts that give them (`JumpTable` and
  `ValueAnalysis` from the machine code; `InterruptTable`, `StageTwoEntry`,
  `WakerVtables`, `FieldType` from outside it) and what they assume. Each root counts its sites by fact; the totals
  count the distinct sites by fact and by assumption, `none` for proven
  sites. The reports read the bounds the gates computed and change no
  verdict.
- `allocated_stacks`: linked CPU0 task stack, each hart's linked IRQ range and
  the bootstrap stack, in bytes.
- `bounds`: each task root, IRQ hart/level/exception and nested IRQ total, with
  byte bound (or resolved partial bytes), `complete`, all holes and assumptions,
  and the largest reachable own frame (including off-critical-path frames).
  A hole retains its demangled containing function, full DWARF inline chain,
  function-relative offset and reason; address movement alone does not turn it into a different hole.
- `frames`: every `.stack_sizes` record under each full demangled function
  alias, preserving concrete generic arguments. Repeated names retain sorted
  individual sizes instead of silently overwriting an implementation.
- `types`: DWARF aggregate layouts (struct/enum/union/class, tuple, future and
  enum variant views) and arrays of at least 256 B. Keys retain enclosing named
  scopes and concrete generic arguments from rustc; arrays use `[element; N]`.
  DIE offsets are local reference identities, never comparison keys. Repeated
  identical layouts coalesce; ambiguous keys with different sizes fail summary
  generation with examples. Variant views are layouts, not separate allocations.
- `resources`: actual work units and peak admitted working memory of the image
  analysis, their limits/deadline, number of incomplete function facts (including
  the ROM companion) and partial roots. These are the analysis's existing
  counters, not process RSS or summary writer allocations. Following #369,
  local analysis can yield partial facts; operation exhaustion is an error and
  is never reported as a successful partial summary.

`cargo fw sizes` reads two output directories; its thresholds and verdicts are
documented in [the build README](../build/README.md#size-summaries-and-owner-thresholds).
The summary itself never changes a check's verdict.

### PR size table for an agent

Build the same app/analysis and HIL configurations on main and on the PR head,
copying all bundles into `target/size-base` and `target/size-head` respectively.
Keep compiler/profile/flags/configuration identical. Run
`cargo fw sizes target/size-base target/size-head` and put the table in the PR;
a `stop` requires a question to the owner. The build README gives a W3 example.

## Reachability reports

The stack policy's `observed_functions` selects demangled linked function
names, examining every alias and the analysed ROM companions. `*` matches
any substring; every other character is literal. An extending image policy
appends selectors to its chip policy. Empty and duplicate selectors are errors.
The chip selectors also apply when an image defines an independent stack
layout instead of extending the chip policy, as W3 does. Report selection is
the union of chip and image selectors, deduplicated before matching.
The chip policies select `core::fmt::write`, `Formatter` methods, the runtime
`Argument::fmt` entry, debug builders, `Write::write_fmt`, and the `fmt`
implementations of Debug, Display, Binary, Octal, LowerHex, UpperHex, Pointer,
LowerExp and UpperExp. Selectors are general and may watch other functions.

`runtime-reachability.txt` reports CPU0 and each task root (a recorded task
poll boundary, `__OER_TASK_POLL.*`) as T0, T1, ... in address order.
`interrupt-reachability.txt` reports each hart/priority and its exception
context with the same context-specific resolutions as the interrupt bound.
`bootstrap-reachability.txt` reports the bootstrap root.
Probe builds write `runtime-reachability.txt` below
`target/fw/probes/<package>/<binary>/`. They report each present boot entry
(`runtime_main`, `_start_rust`, `main`, `_start`, deduplicating addresses),
the task polls and, where a staged interrupt table is linked, IRQ contexts.
Without that table the report explicitly makes no IRQ completeness claim.
These files are separate from stack, frame and placement reports and do not
introduce a formatting gate or change stack limits.

For each root and selected implementation the verdict is:

- `reachable`: the bound's `reached` inventory contains its address. One
  witness lists the root and each traversed caller, callsite and callee.
- `proven-unreachable`: the address is absent and the root has no holes.
- `unknown`: the address is absent and the root has holes. The report lists
  all those holes once per root, with their sites, owners and reasons.

An unmatched selector is explicit (`linked=0`) and follows the same negative
verdict rule. Inlined bodies are not separate linked function entries.
An unknown verdict never claims that a function behind a hole is unreachable.
Assumptions remain visible, including the executor invariant: absence of holes
is completeness under the bound's recorded assumptions.

The root summary counts one witness per reached implementation, deduplicating
aliases and overlapping selectors. It does not count every possible call
chain. The stack walk retains one predecessor at first discovery; a witness
follows these parent links and is neither a shortest-path search nor the
deepest stack path. Reporting does not traverse call edges again. Predecessor
storage is admitted through `Ledger` beside the retained image analysis,
using the existing memory limit. The reports carry `analysis_resources`, including the accounted peak with
witnesses. Charged work describes the original image analysis; bounds retain
their existing separate work/deadline guards.

Schema 2 additionally records section classes (`code`, `read_only`, `writable`,
`filler`), input-free filler ranges within sections, `NOBITS`, totals by class/memory, final binary sizes, and the
byte-verified embedded-runtime range in bootstrap. Net code excludes filler
and that duplicate payload. Hole comparison uses the multiset of function,
inline chain and reason; offsets order repeated sites. See the
[comparison and refresh rules](../build/README.md#size-summaries-and-owner-thresholds).

Stack and reachability text reports begin with one `analysis_resources` JSON
line. The `image-checks` profile preserves 1 GiB admission, one billion work
units and 300,000 ms per analysis budget. The line records effective limits,
local formulas, charged image-analysis work, peak admitted bytes and typed-limit
events/affected functions/output records. Preparation and final value passes
contribute events while the same physical function counts once. Output counts
cover explicit reasons, including the expression-limit summary line in stack
reports; unknown bounds do not acquire an inferred resource cause. Limits
without typed local reasons are named in `unaccounted`. These headers do not
change stack bounds, holes, verdicts or allocated firmware stacks.
