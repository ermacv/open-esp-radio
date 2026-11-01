# Image pipeline

`oer-image-build` is the one image pipeline of the repository: `build(ImageSpec)
-> ImageBundle`. `cargo fw` ([tools/image/cli](../cli/README.md)) and the HIL harness
(`tools/hil/harness`, for a scenario's image) build every image here; nothing
else compiles, checks or encodes one. `images` resolves an image by its
directory in `firmware/apps` or `hil-scenarios/images` and gives its spec and
bundle to both. Every image builds with the `firmware` profile against
the workspace's one `Cargo.lock` (`--locked`).

## Code-generation preconditions

The compiler setup in `src/compiler.rs` pins the conditions required by the
[DWARF Waker-slot proof](../stack/README.md#code-generation-preconditions)
for every Rust image build, including bootstrap and probes:

- `-Zmerge-functions=disabled` keeps distinct function origins from becoming
  one function with only one surviving DWARF description.
- `-Cllvm-args=-enable-machine-outliner=never` disables machine outlining
  explicitly, independently of optimization level, CPU and target features.
  The pinned RISC-V target's `+m,+a,+c,+f` features need no outlining exception.
- `pick-merged-source-locations` must not be set, even explicitly to false.
- No `--icf` option may reach LLD, including `--icf=none`; its default is off.

The pinned stable toolchain accepts the unstable MergeFunctions flag through
this setup's image-only `RUSTC_BOOTSTRAP=1`, alongside the existing stack-size,
move-limit and shared-generics flags. A compiler-driven test checks that two
identical callback wrappers with different source traits keep distinct code.

Cargo's encoded rustflags take precedence over ordinary `RUSTFLAGS`. The
setup reads the effective input, appends the required flags, validates the
whole argument list, and passes it as `CARGO_ENCODED_RUSTFLAGS`; probe cfgs
extend the same list. Argument boundaries are retained, including spaces in
LLVM options and linker paths. Conflicting settings return a typed
`oer_image_checks::codegen::CodegenPrecondition`, even if a later flag would
override them. Explicit passes that re-enable merging or outlining are also
refused. The [image linker](../linker/README.md) checks the actual expanded
link command, including build-script options and nested response files.
These checks are unconditional; selecting reported image gates does not
relax them.

## Contract

An `ImageSpec` names the purpose (`Flash` or `Analysis`), the tree
(`root`: a checkout or a worktree), the
chip, the application crate (package, binary, features), the stack
policy, the layout seed, the variables the runtime's build sets or removes
(runtime configuration), the caller's builder packages and
extra policy files, the bundle directory (every build compiles in the checkout's
`compile_cache()` of its instrumentation classes) and the checks (`Gates` of `oer-image-checks`): which
run, what the interrupt checks require (`Required::Product`, or
`Diagnostic` for diagnostic images), an optional audit of the runtime ELF (HIL's
observer placement), and which requested checks must pass (`Enforced`).
`Enforced::All` enforces every requested check, including the caller's audit;
`Only(BTreeSet<Check>)` enforces only the named built-in checks;
`Reported` turns failures into warnings containing `reported only`.
`Gates::all` and `Gates::of` select `All`; a default `Gates` selects `Reported`
and requests no checks.

Every staged build additionally requires the bootstrap's cache-off placement
audit, independently of `Gates` and `Enforced`. All linked executable sections
and ordinary constant sections must be in the chip's internal SRAM. The
explicit flash payload and tuning-reference sections remain flash sources.
Every allocated flash section must have an exact allowlisted metadata/payload
name and non-writable, non-executable flags; unknown sections fail regardless
of their names. See the [image checks](../checks/README.md).
This audit runs on the actual bootstrap ELF, including a Cargo-cached ELF,
before encoding any flash files; a diagnostic `run` cannot turn its failure
into a warning. `bootstrap-placement.txt` records the result.
The encoded ESP application's mapped segment count must also satisfy the
chip's `[boot-chain] mapped-segments` loader contract; `bootstrap-loader.txt`
records it before any application bytes or bundle are published. On S31,
payload and tuning reference supply two read-only mappings in the unified
D/I window. This grants no executable-flash exception.

| Caller | Enforcement | Required checks |
| --- | --- | --- |
| `cargo fw build`, including `--analysis` | `Only({Interrupts})` | Interrupt stacks; frames and panic budgets in runtime/bootstrap are reports |
| `cargo fw run` | `Reported` | None |
| HIL image builds | `All` | Every requested check and audit |

App builds report frame sizes, panic inventories, task-stack bounds, the
bootstrap-stack bound and placement. Frame and panic thresholds produce
warnings under every enforcement policy, including HIL's `All`. Missing or
malformed panic-budget metadata is a configuration error.
The staged build/run pipeline also enforces the hart-local critical-section prerequisite:
`[package.metadata.open-radio] critical-section-hart = "boot"` selects the boot
hart from facts; an explicit hart ID selects that used hart. The image's declared
foreign task, interrupt-table handler and exception roots must have complete
resolved paths excluding linked critical-section entries, including inline copies.
`critical-section.txt` records the result.
This prerequisite and unused-hart containment apply even when optional checks
are reported or absent. `[package.metadata.open-radio] harts` (hart IDs or
`"boot"`, the boot hart required; absent, the boot hart alone) names the harts
an image runs: each declared secondary uses its stack policy's launch template
(`StackPolicy::for_image`), and every undeclared template stays unused. The
image linker's analysis specification carries the same set.
`cargo fw probe` emits reachability reports only, even for runtime-linked binaries;
it does not run this gate.
`cargo fw flash` uses an existing bundle without rerunning its checks.
For app `build`, a `partial + ?` interrupt stack, an exceeded budget or an
unadmitted assumption fails the build and CI; a bound conditional only on
the admitted executor invariant passes with a warning. An image that declares
`task-stack = "proven"` under its `[package.metadata.open-radio]` is held to
more: any unresolved path in a task stack's bound fails its build, with the
unresolved sites listed, and `runtime-stack.txt` says the image sizes the
stacks from their bound. A task stack whose image declares it proven is sized
at link time to its bound (proven, or conditional on an admitted assumption)
plus its policy's minimum free bytes, by the image linker (see the
[image linker](../linker/README.md)); `runtime-stack.txt` names the source of
the size. A policy that also sets `allocation_bytes` is an error. Any other
value of the key is an error. HIL retains its
`Product`/`Diagnostic` interrupt policy according to image class: diagnostic
images report strict/executor-task code placement violations and breaches of the
hardware class as warnings; other images
enforce them.

The chip profile's boot kind selects the pipeline:

| Boot | Module | Bootloader and partition table |
| --- | --- | --- |
| `staged` (esp32s31) | `staged` | The ROM-readable DIO bootloader from the `espflash` library's resources, the partition table of the flash map's CSV, the `ota_0` selection |
| `esp-idf-bootloader` (an ESP-IDF catalog image) | `esp_idf` | The catalog project's own bootloader and partition table, built against the pinned ESP-IDF (`esp_idf::catalog`, `esp_idf::idf`); its `flasher_args.json` must place them at the flash map's offsets; the flash map is the chip profile's, or for an ESP-IDF peer chip without a profile (`esp32c5`) its `chips/<chip>/platform/flash.toml` |

An application package may declare instrumentation classes in its manifest,
`[package.metadata.open-radio] instrumentation = ["trace"]`: `trace` (what
happened and execution cost: values and events collected by the image) and
`debug-control` (knobs that force execution paths). The
pipeline reads the declaration with `cargo metadata` and compiles the
runtime and the bootstrap with the classes' cfgs (`oer_trace`,
`oer_debug_control`) in a compile cache of their own
(`target/fw/cargo-<class>-…`); an image that declares none compiles none, in
`target/fw/cargo`. Probes are no class: `cargo fw probe` sets
`cfg(oer_probe)` for its own builds alone, no production crate reads it, no
crate has a probe feature, and production has no item only a probe uses.

An image build fills an `ImageBundle` (format and contents:
[`oer-image-bundle`](../bundle/README.md)) with the files below the bundle's
own, and records `purpose: Flash` or `Analysis`.

Every link using the staged runtime script (`-Truntime/link.x` from
`configure_runtime`) requires an `OER_IMAGE_INTERRUPTS` specification and
runs the final check's analysis after the first link. It links once more
to place strict cached code in SRAM and size IRQ stacks independently by hart
([image linker](../linker/README.md)). The final ELF is analysed again before
fingerprinting; its bound plus the rounded-up contract margin must fit the
actual allocation minus its guard. A bound change above 64 bytes fails,
without another link or fallback to the maximum. Without the specification the linker
fails with the bin name and instructions to use `cargo fw build`,
`cargo fw probe` or `cargo hil`. Links using another script ignore a supplied
specification and produce no analysis fragment or placement record.
`strict-text.x` is included inside `.isr.text` and `hot-text.x`, the
functions of the image's hot-code list (`hot-code = "<file>"` under its
`[package.metadata.open-radio]`, a path beside its manifest), inside
`.hot.text`; empty fragments preserve the first image. A cached runtime link is reused only when the SHA-256 fingerprint
in `strict-placement.toml` matches the final ELF, image linker, specification
and analysis inputs (the hot-code list among them), and both fragments match
the placement record. Only a
mismatch invalidates the runtime package through Cargo and rebuilds it once;
dependencies remain cached. A second mismatch fails the build. The bootstrap
has no such invalidation or interrupt analysis; its cache-off placement audit
always runs before encoding. The existing interrupt check remains final.

`cargo fw probe <package>` supplies the same specification, with the harts
the probe declares, and uses the same fingerprint check and single rebuild. It keeps `image-linker.toml`,
`strict-text.x` and `strict-placement.toml` in
`target/fw/probes/<package>/`; compilation stays in the separate
`target/fw-probe/` cache with `cfg(oer_probe)`. Cargo's executable artifacts,
including Fresh binaries, identify which linked ELFs carry the runtime
script's `__runtime_image_start` boundary and need fingerprint validation.
Standalone probe bins in the same package do not run the analysis or overwrite
the runtime's fragment and placement record.

A build takes its output's lease (`.<name>.build.lease` beside it: a second
build of the same output fails at once), fills the sibling staging directory
`.<name>.staging` and publishes it whole ([publication](../bundle/README.md#publication)).
A build that fails publishes nothing: the staging directory keeps the failed
build's `build.log` and `checks.json` until the next build of that output.
`around` makes the bundle of an application encoded earlier (a run's archive,
a vendor ESP-IDF build), so replays and foreign applications are flashed the
same way.

## Flash map

Each chip's `chips/<chip>/platform/chip.toml` `[flash]` table
([`oer-espressif-facts`](../../../chips/espressif/facts/README.md)' `FlashMap`,
with the flash size of the chip's module) holds the
bootloader, partition-table, application and OTA-data offsets and the
partition-table CSV path (`chips/<chip>/platform/partitions/`). The S31
uses staged boot; its profile supplies the staged flash map. An ESP-IDF peer chip without a profile (`esp32c5`) names the same map in `chips/<chip>/platform/flash.toml`; a chip has one or the other. The flash map is host data: no target code reads it (the
bootstrap finds its payload through the flash MMU). The staged pipeline
refuses a flash map whose application or OTA-data offset is not the
partition table's.

## Encoding

Every flash file is encoded at build time with the `espflash` library
(`encode`): the application in QIO at 80 MHz for 16 MB with 64-KiB MMU
pages, the bootloader separately in the ROM's DIO (`rom_bootloader` checks
the segment checksum and digest). The local `encode::partition_table` module
reads and writes partition tables; `espflash` remains the image encoder and a
stand host tool, including its transitive dependencies. The build needs no
`espflash` executable.

The partition-table module supports the pipeline's app/data CSV tables with
explicit offsets and sizes, decimal/hex numbers and K/M size suffixes. Named
subtypes are `factory`, `ota_0` through `ota_15`, `nvs`, `ota` and `phy`;
numeric byte subtypes are retained. Flags are `encrypted` and `readonly`.
Binary entries use bounded `oer-wire` cursors and typed fields. The writer emits
an MD5 record and erased padding; the reader accepts the format's optional MD5,
verifies any MD5 present, and requires an erased end marker. Unknown flag bits
and all label bytes survive a binary round trip. Invalid input returns typed
errors, including unknown types, overflow, overlaps and malformed records.
Format facts are sourced beside the implementation from the ESP32-S31 ESP-IDF
pin in [`chips/esp32s31/vendor/artifacts.toml`](../../../chips/esp32s31/vendor/artifacts.toml).

The application must fit its partition; at 90 % the build warns.

## Gates

The image build pipeline invokes the gates in `oer-image-checks`.
[`oer_image_checks::stack`](../checks/src/stack.rs) implements the runtime
task-stack gate (`audit_runtime_stacks`) and the bootstrap stack gate
(`audit_bootstrap_stack`). The checks analyze each ELF once with
`oer-image-stack`, the pinned ROM and its summaries, and resolve
indirect sites with the same facts (waker vtables, function-pointer field
types). The runtime's task and interrupt gates share that analysis.
The `frames` report shares it too, and the bootstrap's frame and stack checks
share one bootstrap analysis. Every record in `Image::stack_sizes` is checked,
whether or not the function is reachable, against the same `max_move_bytes`
that configures the compiler's move reporting threshold. Oversized frames
and frames exceeding their exception ceilings produce warnings. Invalid or
stale exception declarations remain configuration errors.
Exceptions in `[[frame_exceptions.runtime]]` or `[[frame_exceptions.bootstrap]]`
name one exact demangled implementation without hashes, with concrete generic
arguments, its current-code reason and an initial ceiling of its measured
frame plus 64 bytes. A key matching multiple implementations or none fails; aliases
at one address count once. A frame at or below the move limit makes its
exception stale and requires removal. Ceilings are reporting references.
An extending policy can append exceptions without
changing its base's other settings. Absent functions must not retain exceptions.
The [checks README](../checks/README.md) specifies the key and policy format.
The separate frame reports list counts, current frames, ceilings, spare bytes
and reasons, including warnings or configuration errors. This report changes no compiler
flags, frame coverage, IRQ guards, stack allocations or stack bounds.

The policy is a chip's `chips/<chip>/platform/stack.toml` (schema 6), extended
by the HIL target's policy. A secondary hart runs only in an image that
declares it, from its launch template's root (the image's
`secondary_entry!`), where it may run its own executor's tasks. Each stack names the function that
runs on it from its top (its root, by symbol), its storage (a sized symbol, or a
linker script's bottom and top symbols) and `minimum_free_bytes`: the root's
worst-case call chain (`Analysis::bound_with`) must leave the storage that
many bytes. The runtime's root is `runtime_main` on the boot hart's task stack, which
the platform runtime's assembly enters at the stack's top; the bootstrap's is
the staged bootstrap's `bootstrap_main`. A root the image lacks fails, as does a stack
without storage.
Unless the image declares `task-stack = "proven"`, the boot hart's task bound
may be `partial + ?`: an executor's task polls and trait
objects leave sites no fact resolves. It passes with a warning when the part
it proves fits, and the report lists every unresolved site with its reason
and function; runtime stack painting, with the same reserves (the build hands
them to the firmware as `OPEN_RADIO_*_STACK_MINIMUM_FREE_BYTES`), checks the
exercised call chains. Secondary task roots require complete bounds; unused
harts link zero bytes for both stack kinds.
The gate also fails an ELF without `.stack_sizes`
records and every function of `oer-image-stack`'s inventory without a record
that `stack-coverage.toml` does not review exactly once (assembly, vector data,
compiler runtime or a linker script's section label; a review supplies no
frame). The analysis of one image runs within
`oer_image_checks::stack::budget`: 10^9 work units, 512 MiB of working memory
(everything the analysis of the image and its ROM holds, as the budget's
rustdoc lists it) and 5 minutes, chosen from the measured
analysis of every image class; running out fails the gate
with `ResourceLimited`. Reports go to
`runtime-stack.txt` and `bootstrap-stack.txt`: what the analysis used of its
work units and working memory, coverage, then per stack its bound (proven,
conditional or partial), budget, headroom, deepest path and unresolved
sites.

[`oer_image_checks::interrupts`](../checks/src/interrupts.rs) implements the
runtime's interrupt gate. It bounds each hart's interrupt stack from the
shared runtime analysis, using the chip profile's `[rom]` ELF and reviewed
summaries and its `[memory]` map and `[interrupts]` contract. The ROM comes
from the chip's pinned vendor artifacts; a missing or changed ROM is an error.
The analysis reports every hart as a bound or `partial + ?` with its holes;
the gate's policy, `Required`, decides what passes: `Product` (product images)
fails unless every bound is known, `Diagnostic` (diagnostic image classes,
whose observers the product does not carry) also passes a `partial + ?` hart
with a warning. Either way the bound, or the proven part, must fit the usable stack
with the contract's margin, and every table entry's slot symbol and the
handler it calls must lie in SRAM (the slot's own direct calls: a call site no
inlined function owns, by the DWARF). A hart with a bound is `proven`, or
`conditional` on the assumptions it names; the gate admits only the executor
invariant (`ADMITTED`), each assumption until a change proves it. Every copy
of a CLIC level or route writer must run where the contract's
`LEVEL_WRITERS` allows, and no handler may lower the running level. Over the
code every interrupt level reaches, a floating-point instruction fails (the
handlers run with the FPU off); calls into the panic machinery and functions
in cached memory (flash, PSRAM) are listed. It writes all of this, each
hart's levels and their critical paths to `interrupt-stack.txt`.
Each hart's exception bound (including its entry frame) plus the largest
own function frame reached at any of its levels, including the exception
and executor tasks, is compared with that hart's `irq-stack-guard-bytes` in the chip's
`chip.toml`: S31 hart 0 has 4096 bytes and hart 1 has 1024.
Guard findings are reported, never gates in build, run or analysis. The image
linker exports the actual physical guard as `__runtime_cpu{hart}_irq_stack_guard_bytes`;
the runtime reads that absolute symbol and arms a trigger only for a nonzero guard.
The IRQ stacks occupy a separate NOLOAD section at the start of internal
SRAM, before ISR code. A placed guard must be NAPOT-aligned at its stack bottom;
when the full guard cannot fit beside the bound plus margin within the maximum,
or contiguous stack placement cannot align it, it is omitted and reported.
There is no shared guard-size constant. `irq-stack-bytes` is a maximum:
the bound plus margin must fit without guard bytes. Proven product stacks use
`align16(ceil(bound * 1.10)) + guard` when the full guard fits, with S31
hart 0 additionally aligned to hart 1's guard. Partial, diagnostic and probe
stacks retain the effective contract maximum. `interrupt-stack.txt` reports
the bound, percentage margin, guard, allocated bytes and source per hart;
the usable budget is the actual linked allocation minus that hart's guard.
The independent IRQ `minimum_free_bytes` reserve was removed from stack
policies; the chip contract's percentage is the sizing margin.
The debug trigger
catches a write into the guard, not a change of `sp`: a function may lower
`sp` by its whole frame before writing, or skip the guard with a larger frame.
The report lists the guard, exception bound, largest frame and function,
their sum and the spare bytes for each hart. A frame on a path other than
the deepest path still counts: reachability is the union of `Bound::reached`
at every level and the exception. Both `Required::Product` and
`Required::Diagnostic` report coverage violations and functions with unknown frames;
negative spare remains visible in the table.
Its placement summary lines separate strict code in cached memory by hart/level
from executor task code in PSRAM and name the task roots. A level's class is
its table entries' (`oer_espressif_interrupt_table::Class`): the level of an `Executor`
entry is the executor's, every other level is hardware. Only an executor's
level has a task boundary; it comes from
the executor's task poll sites (`task_poll_sites`) through `Analysis::reached_cut`; each level
uses the same interrupt routes as its stack bound. Strict code, including the
exception and panic paths, must lie in SRAM/ROM; executor tasks may also use PSRAM,
while cached flash fails on both sides under `Required::Product`.
`Required::Diagnostic` reports these placement violations and code outside
the permitted regions as warnings, one per kind and hart/level, with function
and byte counts and a reference to `interrupt-placement.txt`. Handler entries
outside SRAM, cached panic code and cached panic-hook data still fail under
both policies. A function with any strict path keeps the strict requirement.
The summaries also count sized data objects referenced
statically from resolved handler code and usable/placed/free SRAM. Full lists in
`interrupt-placement.txt` sort cached functions by size with `strict`/`task`
classification, total them by the first path component of their demangled names,
show strict SRAM/ROM/cache totals and root paths to strict cached functions,
and name every data reference and object.
Code targets are excluded from the data count; aliases with the same address and
size count once, and an interior address counts the whole object. References
without a sized data symbol remain visible and contribute no invented size.
SRAM usage is the union of the placement contract's SRAM ranges and allocated
SRAM sections, including NOLOAD interrupt stacks and DMA. Overlaps count once;
the contract's upper boundary is the boot chain's reserve, below the ROM's data
(the chip facts' `[boot-chain] application-sram-end` and `[rom] sram`). These are static byte counts of
resolved paths, without relocation alignment or dynamically addressed data;
data and SRAM measurements do not participate in a gate's verdict.
Independently of the installed interrupt handlers, it also follows linked
compiler panic entry points, the platform panic handler and the optional
image panic hook. A resolved callee in cached memory fails the build under
both interrupt policies, so a panic helper or hook cannot silently move to PSRAM.
The hook and its callees also must not statically reference cached memory:
the retained ELF relocations reveal globals, lookup tables and anonymous
constants introduced by the compiler, including constant-copy initialization.

The image compiler flags have one owner, `compiler::configure`, which a
stack policy parameterizes through `compiler::of_policy`:
`-Z emit-stack-sizes`, `-Z move-size-limit` (the policy's `max_move_bytes`) with `-W large-assignments`,
`-Z share-generics=y` and the C/C++ `-fstack-size-section`. Moves above the
limit emit compiler warnings; the limit does not reject them. No
`.cargo/config.toml` of the repository carries Rust flags (Cargo reads those
from the directory it runs in, not from the manifest's, so image builds from
the repository root would never see them). It sets `RUSTC_BOOTSTRAP=1` on the
image's Cargo command alone, only for those `-Z` flags on the exact stable
toolchain `rust-toolchain.toml` pins. Images build without frame pointers;
the staged runtime records panic diagnostics for next-boot reporting
without a frame-pointer walk.

The same setup builds the pinned [large-value reporter](../large-values/README.md)
and installs it through `RUSTC_WRAPPER`, with an exact workspace-manifest
filter from `cargo metadata`. Dependencies outside the workspace, host build
scripts and proc macros run ordinary rustc with unchanged arguments. The
wrapper preserves Cargo artifact hashes and the image compiler flags.
`rustc-dev` is a component of the pinned toolchain for both host and chip CI.

CI uploads the `*.txt` and `*.json` reports of every image it builds as its
run's `image-reports` artifact, also when a later build fails.

Every staged app and HIL image has `large-values.json` and `large-values.txt`
beside its ELFs; probe reports are in `target/fw/probes/<package>/`. These
report monomorphized MIR locals, temporaries, arguments, return slots and
call operands above the policy's `max_move_bytes`, with full instances,
concrete types, sizes and outer macro call sites. Static allocations are
excluded. The report is separate from image checks and does not reject an
oversized value. It describes pre-LLVM values, including values LLVM later
removes; it supplies neither final frames nor call-chain bounds.

Coverage lists the checked workspace units, host units and unchecked
nonworkspace dependencies for runtime and bootstrap separately. Hashed
sidecars remain at the compiler artifacts. Each image reads Cargo's actual
artifact messages, including Fresh units, and checks the tool identity,
limit and workspace manifest. Cargo metadata supplies the configured
`build_directory` for dependency outputs and fingerprints, independently of
the target directory containing final uplifts. Sidecars must also postdate every compiler
output, so an ordinary rebuild after a source edit cannot reuse older facts. Cargo's binary uplifts select the hashed sidecar by file identity; copies
across filesystems require a unique byte-identical hashed output, rejecting
ambiguous variants. Copy timestamps do not replace compiler-output freshness. The
full ELF digest verifies the contents. Even byte-identical feature variants
retain their own facts.
Missing or stale facts invalidate only the affected workspace packages'
fingerprints in that image cache, then rerun Cargo once. A remaining missing
report or inspection error is explicit. The image JSON envelope and each
crate's compiler facts have independent schema versions.

The stage-two header, checksum and address map come from the
[platform layout](../../../chips/espressif/staged-layout/README.md), which application
build scripts also use to configure the linker.

The staged runtime's placement audit (`staged::placement`) checks the
PSRAM/SRAM placement contract of the
[platform layout](../../../chips/espressif/staged-layout/README.md) and that each
PSRAM trap and interrupt entry first swaps to its SRAM stack; the caller's
audit runs beside it.

A stack policy that names no stacks sets the move and per-function frame limit:
the image is compiled with every image flag and has no whole stack to bound. Staged S31 images
name their stacks and pin their ROM inputs.

## Comparing images

`compare::compare_elf` compares two linked RISC-V images function by
function modulo placement, over the `oer-analysis-elf` symbols and the decoded
listings of `oer_analysis_riscv_lift::listing`: every address an instruction forms
becomes symbol+offset, legacy mangling hashes and LLVM clone numbers are
dropped, identical-code-folded names pair by body, and a `Review` applies
reviewed aliases and scheduling ties.

## Exclusion

Build exclusion has one mechanism, the foundation's file lock
(`oer_os_process::lock::FileLock`, shared or exclusive): one build at a
time in the shared compile cache (`exclusion::compile_cache`, `<cache>/build.lock`,
waiting, held from the Cargo run until the ELF is copied out), one
build per output (`exclusion::Staging`, refusing), the host's image build
slots (`exclusion::slot` under `host_build_root()/tokens`: half the cores, at
2 GB each), and the ESP-IDF cache (exclusive while its tree and tools change,
then converted to shared for the builds).

Run host regressions with `cargo test -p oer-image-build`.

## Size summaries and verdicts

App builds (including `--analysis`) and HIL image builds publish `sizes.json`
next to `runtime.elf` and `bootstrap.elf`, listed in `bundle.json`'s reports.
It observes the checks' existing image analysis; it changes neither the
compiler flags, the ELF nor the checks' verdicts. The versioned format is
specified in [the checks README](../checks/README.md#size-summary-format).

```console
cargo fw sizes target/before target/after
```

This command recursively finds summaries in two output directories and builds
nothing. Image sets and provenance must match: chip, package and binary, target,
compiler, profile settings, flags, features, purpose, configuration and layout
seed. Commit and dirty status identify the inputs; they need not match. Stack
policy source digests are retained so a policy edit remains reviewable.
Configuration values are SHA-256 digests, including explicit removals; stand
credentials are never recorded as plaintext. Checkout paths in compiler flags
are normalized to `$ROOT`.

The comparison prints a table per image, byte changes for each ELF, new and
removed functions/types, analysis resource use, per-image verdicts and the
overall verdict. `same` means no size, allocation, bound, hole, assumption or
section change; differing provenance commits and resource consumption alone do
not change it. `report` means changes that are only reported; both exit 0.
Errors (unreadable input, mismatched image sets, incompatible provenance)
exit 1.

The verdicts follow the "Sizes" rule in
[`CLAUDE.md`](../../../CLAUDE.md) and are decided in
[`sizes/compare.rs`](src/sizes/compare.rs). Every growth is `report`: the
numbers stay in the table and nothing stops. Growth of an SRAM section or of an
allocated stack (task or interrupt), and a section moving into SRAM, also get a
`GROWTH:` line.

Code growth is the **net** change in executable and read-only image contents
across the runtime and bootstrap, excluding linker filler and byte-verified
embedded copies of the runtime; shrinking contents offsets growth elsewhere,
including a move between sections. A writable PSRAM section is RAM in the
table.
Input-free linker filler carries no owned RAM or image-content budget. A new hole
on an interrupt path already fails `cargo fw build`. Executable and read-only
SRAM also contribute to net code. `NOBITS` reserves occupy RAM where
applicable but are not image contents.

Schema 2 records each section's class (`code`, `read_only`, `writable`, or
`filler`), memory kind and flags. The staged runtime's input-free zero tail is
recognized by the existing linker symbols `__runtime_data_load_end`,
`__runtime_image_start`, and `__runtime_payload_end`: the whole section must
span aligned data end to max(start + 1, image start + 64 KiB), contain only
zero bytes, and contain no sized input symbols or relocations. Bootstrap's
input-free page-alignment gap is likewise measured from
`__psram_runtime_payload_flash_end` to `__flash_tuning_reference_start`, verified
against ALIGN(4096), zero bytes and absence of input symbols/relocations within
that range. These partial-section filler ranges are recorded separately;
section names are not used. The bootstrap's embedded runtime is found by matching all bytes
of `runtime.bin` against its allocated read-only sections; its address, size,
section and SHA-256 are recorded and those duplicate bytes are excluded from
code. Bootstrap's own code and read-only data remain counted. `runtime.bin`
and `application.bin` sizes are printed separately.

A hole's identity is its demangled function, complete DWARF inline chain and
reason. Comparison retains the count of each identity: offsets only order
repeated sites. Moving a call inside a function adds no new hole; adding a
second call of an existing identity does. Every new occurrence is reported.
Completeness and assumptions remain visible in the report. New functions/types
are listed separately as additions/removals; a missing baseline is not a measured
zero-byte layout. Growth comparisons use matching keys. Types crossing the
256 B inventory boundary are therefore visible as new/removed. Multiple frame
implementations sharing a demangled name retain their sorted individual sizes.

### PR size table for an agent

When a targeted image comparison is authorized, build its base and head in
two output directories, with the same compiler,
flags, image configuration and profile. For example, from clean main build
`cargo fw build radio-root`, then copy `target/fw/radio-root` to
`target/size-base/radio-root`. Switch to the PR head, repeat the build and copy
the output to `target/size-head/radio-root`. Include the selected image's
bundles, or reuse captured bundles with matching provenance. Then run:

```console
cargo fw sizes target/size-base target/size-head
```

Include its table and verdict in the PR. Growth is reported; image-specific
limits remain the gates described above.
Different summary schemas are incompatible, even if both files would otherwise
have matching provenance. To reanalyse saved bundles without compiling or
linking, write new summaries to a separate directory:

```console
cargo fw sizes-refresh target/size-base target/refreshed-base --source-root /path/to/base-configuration
cargo fw sizes-refresh target/size-head target/refreshed-head --source-root /path/to/head-configuration
cargo fw sizes target/refreshed-base target/refreshed-head
```

The input can be one bundle or an output tree. It is read-only; refreshed output
contains only new `sizes.json` files. The command reads the original ELF and
binary files, preserves original build provenance, and uses the usual analysis
and its unchanged budgets. `--source-root` defaults to the current checkout;
it must supply the original chip configuration and stack policy (recorded
policy hashes must match). A separate checkout or an exported source tree can
supply historical configuration. Existing analysis reports are neither parsed
nor changed. Schemas 1 and 2 can be refreshed; comparison accepts schema 2 only.
