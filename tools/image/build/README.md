# Image pipeline

`oer-image-build` is the one image pipeline of the repository: `build(ImageSpec)
-> ImageBundle`. `cargo fw` ([tools/image/cli](../cli/README.md)) and the HIL image
classes (`oer-hil-run`, through `cargo hil image build`, the runner and
`cargo hil images check`) build every image here; nothing else compiles,
checks or encodes one. Every image builds with the `firmware` profile against
the workspace's one `Cargo.lock` (`--locked`).

## Contract

An `ImageSpec` names the purpose (`Flash` or `Analysis`), the tree
(`root`: a checkout or a worktree), the
chip, the application crate (package, binary, features), the stack
policy, the layout seed, the variables the runtime's build sets or removes
(`cargo fw`'s stand identity of `w1`), the caller's builder packages and
extra policy files, the bundle directory (every build compiles in the checkout's
`compile_cache()` of its instrumentation classes) and the checks (`Gates` of `oer-image-checks`): which
run, what the frame and interrupt checks require (`Required::Product`, or
`Diagnostic` for diagnostic images), an optional audit of the runtime ELF (HIL's
observer placement), and which requested checks must pass (`Enforced`).
`Enforced::All` enforces every requested check, including the caller's audit;
`Only(BTreeSet<Check>)` enforces only the named built-in checks;
`Reported` turns failures into warnings containing `reported only`.
`Gates::all` and `Gates::of` select `All`; a default `Gates` selects `Reported`
and requests no checks.

| Caller | Enforcement | Required checks |
| --- | --- | --- |
| `cargo fw build`, including `--analysis` | `Only({Frames, Interrupts})` | Function frames in runtime/bootstrap and interrupt stacks |
| `cargo fw run` | `Reported` | None |
| HIL image builds | `All` | Every requested check and audit |

App builds still report task-stack bounds, the bootstrap-stack bound and placement.
`cargo fw flash` uses an existing bundle without rerunning its checks.
For app `build`, a `partial + ?` interrupt stack, an exceeded budget or an
unadmitted assumption fails the build and CI; a bound conditional only on
the admitted executor invariant passes with a warning. HIL retains its
`Product`/`Diagnostic` interrupt policy according to image class: diagnostic
images report large unexcepted frames and strict/P2 code placement violations as warnings; other images
enforce them.

The chip profile's boot kind selects the pipeline:

| Boot | Module | Bootloader and partition table |
| --- | --- | --- |
| `staged` (esp32s31, esp32c5) | `staged` | The ROM-readable DIO bootloader from the `espflash` library's resources, the partition table of the flash map's CSV, the `ota_0` selection |
| `esp-idf-bootloader` (an ESP-IDF catalog image) | `esp_idf` | The catalog project's own bootloader and partition table, built against the pinned ESP-IDF (`esp_idf::catalog`, `esp_idf::idf`); its `flasher_args.json` must place them at the flash map's offsets |

An application package may declare instrumentation classes in its manifest,
`[package.metadata.open-radio] instrumentation = ["trace"]`: `trace` (what
happened: values and events), `profile` (what execution costs: time,
cycles, load) and `debug-control` (knobs that force execution paths). The
pipeline reads the declaration with `cargo metadata` and compiles the
runtime and the bootstrap with the classes' cfgs (`oer_trace`,
`oer_profile`, `oer_debug_control`) in a compile cache of their own
(`target/fw/cargo-<class>-…`); an image that declares none compiles none, in
`target/fw/cargo`. The probe hooks are no class: `cfg(oer_probe)` belongs to
`cargo fw probe` alone.

An `ImageBundle` is one directory with a `bundle.json`:

| File | Content |
| --- | --- |
| `runtime.elf` | The application ELF (the staged boot's stage-two runtime) |
| `application.bin` | The encoded ESP application image (the staged boot's bootstrap with the packed runtime embedded) |
| `bootloader.bin`, `partitions.bin` | The second-stage bootloader and the binary partition table |
| `otadata.bin` | The OTA selection of the first slot (staged only) |
| `runtime.bin`, `bootstrap.elf` | The packed runtime and the bootstrap (staged only) |
| `placement.txt`, `runtime-stack.txt`, `interrupt-stack.txt`, `bootstrap-stack.txt` | Gate reports |
| `runtime-frames.txt`, `bootstrap-frames.txt` | All recorded frames against the move limit, exceptions with ceilings and spare bytes, Diagnostic warnings |
| `interrupt-placement.txt` | Interrupt code placement, paths to cached functions, static data references and objects, and free SRAM |
| `image-linker.toml`, `strict-text.x`, `strict-placement.toml` | Runtime linker analysis inputs, generated strict-code fragment and the functions moved with their input sections |
| `large-values.json`, `large-values.txt` | Compiler values above the move limit and workspace coverage |
| `build.log` | Every step's standard error |

The manifest uses schema 7 and records `purpose: Flash` or `Analysis`.
Every link using the staged runtime script (`-Truntime/link.x` from
`configure_runtime`) requires an `OER_IMAGE_INTERRUPTS` specification and
runs the final check's analysis after the first link. It may link once more
to place strict cached code in SRAM. Without the specification the linker
fails with the bin name and instructions to use `cargo fw build`,
`cargo fw probe` or `cargo hil`. Links using another script ignore a supplied
specification and produce no analysis fragment or placement record.
`strict-text.x` is included inside `.isr.text`; an empty fragment preserves the
first image. A cached runtime link is reused only when the SHA-256 fingerprint
in `strict-placement.toml` matches the final ELF, image linker, specification
and analysis inputs, and its fragment matches the placement record. Only a
mismatch invalidates the runtime package through Cargo and rebuilds it once;
dependencies remain cached. A second mismatch fails the build. The bootstrap
has no such invalidation or analysis. The existing interrupt check remains final.
Older schemas must be rebuilt; a missing purpose never defaults to `Flash`.
`ImageBundle::load` verifies files for either purpose. The shared flash
snapshot reader refuses `Analysis` before supplying bytes to any USB or
JTAG writer, including `cargo fw` and HIL. Copies made with
`with_application` retain the original purpose.

`cargo fw probe <package>` supplies the same specification and uses the same
fingerprint check and single rebuild. It keeps `image-linker.toml`,
`strict-text.x` and `strict-placement.toml` in
`target/fw/probes/<package>/`; compilation stays in the separate
`target/fw-probe/` cache with `cfg(oer_probe)`. Cargo's executable artifacts,
including Fresh binaries, identify which linked ELFs carry the runtime
script's `__runtime_image_start` boundary and need fingerprint validation.
Standalone probe bins in the same package do not run the analysis or overwrite
the runtime's fragment and placement record.

`ImageBundle::segments` is what a flash writes: bootloader, partition table
and application at the flash map's offsets, the OTA selection last. Board
support writes these files and encodes nothing.

A bundle is published whole. A build takes its output's lease
(`.<name>.build.lease` beside it: a second build of the same output fails at
once), fills the sibling staging directory `.<name>.staging`, and
`ImageBundle::publish` records each flash file's SHA-256 and length in
`bundle.json` and renames the staging directory into place. A build that
fails publishes nothing: the bundle published before stays as it was, and
the staging directory keeps the failed build's `build.log` and `checks.json`
until the next build of that output. `ImageBundle::load` verifies every
flash file against its recorded digest, and a write reads the flash files
once into an `ImageBundle::snapshot`, verified the same way, which it hashes
into its receipt and writes over USB or JTAG. `bundle::around` makes the
bundle of an application encoded earlier (a run's archive, a vendor
ESP-IDF build), so replays and foreign applications are flashed the same
way.

## Flash map

Each chip's `chips/<chip>/platform/chip.toml` `[flash]` table
([`oer-chip-profile`](../../chip/profile/src/lib.rs)'s `FlashMap`) holds the
bootloader, partition-table, application and OTA-data offsets and the
partition-table CSV path (`chips/<chip>/platform/partitions/`). Both S31 and
C5 use staged boot. The [C5 profile](../../../chips/esp32c5/platform/chip.toml)
supplies its staged flash map, and
[`oer-esp32c5-layout`](../../../chips/esp32c5/layout/src/lib.rs) supplies its
board memory map. The flash map is host data: no target code reads it (the
bootstrap finds its payload through the flash MMU). The staged pipeline
refuses a flash map whose application or OTA-data offset is not the
partition table's.

## Encoding

Every flash file is encoded at build time with the `espflash` library
(`encode`): the application in QIO at 80 MHz for 16 MB with 64-KiB MMU
pages, the bootloader separately in the ROM's DIO (`rom_bootloader` checks
the segment checksum and digest), partition tables with `esp-idf-part`. The
build needs no `espflash` executable. The application must fit its
partition; at 90 % the build warns.

## Gates

The image build pipeline invokes the gates in `oer-image-checks`.
[`oer_image_checks::stack`](../checks/src/stack.rs) implements the runtime
task-stack gate (`audit_runtime_stacks`) and the bootstrap stack gate
(`audit_bootstrap_stack`). The checks analyze each ELF once with
`oer-analysis-riscv-stack`, the pinned ROM and its summaries, and resolve
indirect sites with the same facts (waker vtables, IPC posts, function-pointer
field types). The runtime's task and interrupt gates share that analysis.
The `frames` gate shares it too, and the bootstrap's frame and stack gates
share one bootstrap analysis. Every record in `Image::stack_sizes` is checked,
whether or not the function is reachable, against the same `max_move_bytes`
that configures the compiler's move limit. `Required::Product` fails on an
unexcepted large frame; `Required::Diagnostic` warns. Both reject invalid or
stale exceptions and frames exceeding their exception ceilings.
Exceptions in `[[frame_exceptions.runtime]]` or `[[frame_exceptions.bootstrap]]`
name one exact demangled implementation without hashes, with concrete generic
arguments, its current-code reason and an initial ceiling of its measured
frame plus 64 bytes. A key matching multiple implementations or none fails; aliases
at one address count once. A frame at or below the move limit makes its
exception stale and requires removal. Ceiling changes may only decrease the
value, enforced in review. An extending policy can append exceptions without
changing its base's other settings. W1 uses its own policy; W3 keeps the
unconfigured policy separate from `stack-configured.toml` for analysis and
stand configurations, since absent functions must not retain exceptions.
The [checks README](../checks/README.md) specifies the key and policy format.
The separate frame reports list counts, current frames, ceilings, spare bytes
and reasons, including warnings or failures. This gate changes no compiler
flags, frame coverage, IRQ guards, stack allocations or stack bounds.

The policy is a chip's `chips/<chip>/platform/stack.toml` (schema 5), extended
by the HIL target's policy. Every image runs its tasks on CPU0; none starts
the second S31 core, and C5 has one hart. Each stack names the function that
runs on it from its top (its root, by symbol), its storage (a sized symbol, or a
linker script's bottom and top symbols) and `minimum_free_bytes`: the root's
worst-case call chain (`Analysis::bound_with`) must leave the storage that
many bytes. The runtime's root is `runtime_main` on CPU0's task stack, which
the platform runtime's assembly enters at the stack's top; the bootstrap's is
riscv-rt's `_start_rust`. A root the image lacks fails, as does a stack
without storage.
A task stack's bound may be `partial + ?`: an executor's task polls and trait
objects leave sites no fact resolves. It passes with a warning when the part
it proves fits, and the report lists every unresolved site with its reason
and function; runtime stack painting, with the same reserves (the build hands
them to the firmware as `OPEN_RADIO_*_STACK_MINIMUM_FREE_BYTES`), checks the
exercised call chains. The gate also fails an ELF without `.stack_sizes`
records and every function of `oer-analysis-riscv-stack`'s inventory without a record
that `stack-coverage.toml` does not review exactly once (assembly, vector data,
compiler runtime or a linker script's section label; a review supplies no
frame). The analysis of one image runs within
`oer_image_checks::stack::budget`: 10^9 work units, 512 MiB of working memory
(everything the analysis of the image and its ROM holds, as the budget's
rustdoc lists it) and 5 minutes, chosen by the owner from the measured
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
and P2 tasks, must fit that hart's `irq-stack-guard-bytes` in the chip's
`chip.toml`: S31 hart 0 has 4096 bytes and hart 1 has 1024; C5 has 4096.
The image linker exports `__runtime_cpu{hart}_irq_stack_guard_bytes` from
this contract; the runtime reads that absolute symbol to program its trigger.
The IRQ stacks occupy a separate NOLOAD section at the start of internal
SRAM, before ISR code. Linker assertions require each stack bottom to align
to its guard; the runtime also rejects misalignment and programs the guard
at that exact bottom.
There is no shared guard-size constant. Allocated stacks keep their sizes;
the usable budget is the allocation minus that hart's guard.
The debug trigger
catches a write into the guard, not a change of `sp`: a function may lower
`sp` by its whole frame before writing, or skip the guard with a larger frame.
The report lists the guard, exception bound, largest frame and function,
their sum and the spare bytes for each hart. A frame on a path other than
the deepest path still counts: reachability is the union of `Bound::reached`
at every level and the exception. `Required::Product` rejects a guard violation;
`Required::Diagnostic` warns, also naming any function whose frame is unknown.
Its placement summary lines separate strict code in cached memory by hart/level
from P2 task code in PSRAM and name the task roots. The task boundary comes from
`TaskPolls`/`NoTasks` resolution facts through `Analysis::reached_cut`; each level
uses the same interrupt routes as its stack bound. Strict code, including the
exception and panic paths, must lie in SRAM/ROM; P2 tasks may also use PSRAM,
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
the contract's upper boundary already excludes the ROM boot-stack side (the
chip's layout documents its ESP-IDF source). These are static byte counts of
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
`-Z emit-stack-sizes`, `-Z move-size-limit` (the policy's `max_move_bytes`) with `-D large-assignments`,
`-Z share-generics=y` and the C/C++ `-fstack-size-section`. A value larger than
that move limit lives in placed static storage; futures and task arguments
borrow it. No
`.cargo/config.toml` of the repository carries Rust flags (Cargo reads those
from the directory it runs in, not from the manifest's, so image builds from
the repository root would never see them). It sets `RUSTC_BOOTSTRAP=1` on the
image's Cargo command alone, only for those `-Z` flags on the exact stable
toolchain `rust-toolchain.toml` pins. Images build without frame pointers;
`esp-backtrace` then reports a panic's message and location without a
frame-pointer walk.

The same setup builds the pinned [large-value reporter](../large-values/README.md)
and installs it through `RUSTC_WRAPPER`, with an exact workspace-manifest
filter from `cargo metadata`. Dependencies outside the workspace, host build
scripts and proc macros run ordinary rustc with unchanged arguments. The
wrapper preserves Cargo artifact hashes and the image compiler flags.
`rustc-dev` is a component of the pinned toolchain for both host and chip CI.

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
limit and workspace manifest. Sidecars must also postdate every artifact
output, so an ordinary rebuild after a source edit cannot reuse older facts. Cargo's binary uplifts select the hashed sidecar by file identity, and its
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
the image is compiled with every image flag and has no whole stack to bound. Staged S31 and C5
images both name their stacks and pin their ROM inputs. C5 keeps the
4096-byte move limit; its console pins the large serving and watchdog
futures separately before joining their references, avoiding a second move
of the console state.

## Comparing images

`compare::compare_elf` compares two linked RISC-V images function by
function modulo placement, over the `oer-analysis-elf` symbols and the decoded
listings of `oer_analysis_riscv_lift::listing`: every address an instruction forms
becomes symbol+offset, legacy mangling hashes and LLVM clone numbers are
dropped, identical-code-folded names pair by body, and a `Review` applies
reviewed aliases and scheduling ties. `cargo fw compare` calls it;
`compare images` builds both sides first (`oer_hil_image::compare_images`).

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

## Size summaries and owner thresholds

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
not change it. `report` means changes within the owner's allowance. `stop`
returns exit 1; errors (including incompatible provenance) also return exit 1.
The command never asks a question itself: the agent takes a `stop` to the owner.

The thresholds have one definition in
[`sizes/compare.rs`](src/sizes/compare.rs), following
[the owner's rule #375](https://github.com/ermacv/open-esp-radio/issues/375):

> Тип/future/кадр ≤ +64 B, вычисленная граница стека ≤ +64 B (выделенные стеки без изменений), код ≤ ~1 KiB на PR — только сообщить; RAM-секции, выделенные стеки или больше — стоп и вопрос владельцу.

Values/frames and bounds allow at most +64 B each; code allows +1024 B per
image, summed over positive executable-section changes across both ELFs.
Shrinking another section does not cancel growth. Every RAM section is checked
individually, including executable sections in RAM; any growth stops. Any
allocated-stack change (including shrink/removal) or new hole stops.
Completeness and assumptions remain visible in the report. New functions/types
are listed separately as additions/removals; a missing baseline is not a measured
zero-byte layout. Growth thresholds compare matching keys. Types crossing the
256 B inventory boundary are therefore visible as new/removed. Multiple frame
implementations sharing a demangled name retain their sorted individual sizes.

### PR size table for an agent

Build main and the PR head in two output directories, with the same compiler,
flags, image configuration and profile. For example, from clean main build
`cargo fw build w3 --analysis`, then copy `target/fw/w3-analysis` to
`target/size-base/w3-analysis`. Switch to the PR head, repeat the build and copy
the output to `target/size-head/w3-analysis`. For a complete chip table include
every app/analysis bundle and the outputs of `cargo hil images check --all` in
each directory. Then run:

```console
cargo fw sizes target/size-base target/size-head
```

Include its table and verdict in the PR, and take any `stop` to the owner.
