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

An image's `stack.toml` (schema 5) may name exact exceptions:

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

The initial ceiling equals the measured frame. Subsequent policy changes may
only decrease it; review must reject increases. A frame exceeding its ceiling
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

## Size summary format

`sizes.json` schema 1 is published by `oer-image-build` beside the image's
ELFs. The check outcome carries `sizes::Measurements` from the same `Image`
used for checking. Task bounds and context-specific interrupt bounds are
captured when the checks compute them; neither a second analysis nor a second
firmware compilation is run.

The top level contains `schema`, `type_min_bytes` (the constant
`oer_image_checks::sizes::TYPE_MIN_BYTES`, **256 B**), `provenance`, and `elfs`
(`runtime` and `bootstrap`). Provenance contains commit/dirty status, rustc's
verbose version, chip, target, profile/settings, application/features, purpose,
layout seed, configuration digests, compiler flags and stack policy source
paths/digests. Values that may contain stand credentials are hashed.

Each ELF contains:

- `sections`: every `SHF_ALLOC` section, keyed by name, with memory kind from
  the chip map, `executable`, `writable`, address and byte size. `totals` gives
  executable code and allocated RAM separately per memory kind (`sram`,
  `psram`, `flash`; a ROM companion is analysed but allocates no image section).
  Totals sum section sizes; overlapping NOLOAD sections remain separately
  visible and are not a physical free-memory estimate.
- `allocated_stacks`: linked CPU0 task stack, each hart's linked IRQ range and
  the bootstrap stack, in bytes.
- `bounds`: each task root, IRQ hart/level/exception and nested IRQ total, with
  byte bound (or resolved partial bytes), `complete`, all holes and assumptions,
  and the largest reachable own frame (including off-critical-path frames).
  A hole retains its demangled containing function, function-relative offset
  and reason; address movement alone does not turn it into a different hole.
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
