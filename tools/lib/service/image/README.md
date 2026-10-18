# Image pipeline

`oer-image` is the one image pipeline of the repository: `build(ImageSpec)
-> ImageBundle`. HIL image classes (`oer-hil-image`, through `cargo hil
image build`, the runner and `cargo hil images check`) build every image
here; nothing else
compiles, gates or encodes one.

## Contract

An `ImageSpec` names the tree (`root`: a checkout or a source snapshot), the
chip, the application crate (workspace, package, binary, features), the
stack policy, what the interrupt-stack gate requires (`Required::Proven`, or
`Partial` for diagnostic images), the layout seed, local dependency
`Overrides` (`ESP_HAL_ROOT`), the
caller's builder packages and extra policy files, the bundle directory
(every build compiles in the host's one `compile_cache()`) and an optional audit of the runtime ELF (HIL's observer
placement).

The chip profile's boot kind selects the pipeline:

| Boot | Module | Bootloader and partition table |
| --- | --- | --- |
| `staged` (esp32s31, esp32c5) | `staged` | The ROM-readable DIO bootloader from the `espflash` library's resources, the partition table of the flash map's CSV, the `ota_0` selection |
| `esp-idf-bootloader` (an ESP-IDF catalog image) | `esp_idf` | The catalog project's own bootloader and partition table, built against the pinned ESP-IDF (`esp_idf::catalog`, `esp_idf::idf`); its `flasher_args.json` must place them at the flash map's offsets |

An `ImageBundle` is one directory with a `bundle.json`:

| File | Content |
| --- | --- |
| `runtime.elf` | The application ELF (the staged boot's stage-two runtime) |
| `application.bin` | The encoded ESP application image (the staged boot's bootstrap with the packed runtime embedded) |
| `bootloader.bin`, `partitions.bin` | The second-stage bootloader and the binary partition table |
| `otadata.bin` | The OTA selection of the first slot (staged only) |
| `runtime.bin`, `bootstrap.elf` | The packed runtime and the bootstrap (staged only) |
| `runtime-Cargo.lock`, `bootstrap-Cargo.lock` | The effective lock files |
| `placement.txt`, `runtime-stack.txt`, `interrupt-stack.txt`, `bootstrap-stack.txt` | Gate reports |
| `source-inputs.json` | Every repository file the build read |
| `build.log` | Every step's standard error |

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

Each chip's `platform/<chip>/chip.toml` `[flash]` table
([`oer-chip-profile`](../../chip-profile/src/lib.rs)'s `FlashMap`) holds the
bootloader, partition-table, application and OTA-data offsets and the
partition table CSVs (`platform/esp32s31/partitions/`). The flash map is
host data: no target code reads it (the bootstrap finds its payload through
the flash MMU), and the esp32c5, whose partition table comes from its
ESP-IDF build, has no layout crate. The staged pipeline refuses a flash map
whose application or OTA-data offset is not the partition table's.

## Encoding

Every flash file is encoded at build time with the `espflash` library
(`encode`): the application in QIO at 80 MHz for 16 MB with 64-KiB MMU
pages, the bootloader separately in the ROM's DIO (`rom_bootloader` checks
the segment checksum and digest), partition tables with `esp-idf-part`. The
build needs no `espflash` executable. The application must fit its
partition; at 90 % the build warns.

## Gates

Its `stack` module is the stack gate of every image build, with one function
for the runtime ELF (`audit_runtime_stacks`) and one for the bootstrap ELF
(`audit_bootstrap_stack`). Each analyses its ELF once with `oer-riscv-stack`,
the pinned ROM and its summaries, and resolves indirect sites with the same
facts (waker vtables, IPC posts, function-pointer field types). The policy is
a chip's `platform/<chip>/stack.toml` (schema 5), extended by the HIL
target's policy. Every image runs its tasks on CPU0; none starts the second
S31 core, and C5 has one hart. Each stack names the function that runs on
it from its top (its root, by symbol), its storage (a sized symbol, or a
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
records and every function of `oer-riscv-stack`'s inventory without a record
that `stack-coverage.toml` does not review exactly once (assembly, vector data,
compiler runtime or a linker script's section label; a review supplies no
frame). The analysis of one image runs within
`oer_image_check_stack::budget`: 10^9 work units, 512 MiB of working memory
(everything the analysis of the image and its ROM holds, as the budget's
rustdoc lists it) and 5 minutes, chosen by the owner from the measured
analysis of every image class; running out fails the gate
with `ResourceLimited`. Reports go to
`runtime-stack.txt` and `bootstrap-stack.txt`: what the analysis used of its
work units and working memory, coverage, then per stack its bound (proven,
conditional or partial), budget, headroom, deepest path and unresolved
sites.

Its `interrupt_stack` module is the interrupt half of the runtime's gate: it
bounds each hart's interrupt stack from
the runtime ELF, the pinned ROM ELF (`rom` of
`verification/esp32s31/artifacts.toml`, from the vendor store: `cargo verification
fetch esp32s31 --artifact rom`; a missing or changed ROM is an error)
and its reviewed summaries, with the platform's interrupt contract
(`oer-esp32s31-platform-layout`'s `interrupts`). The analysis reports every
hart as a bound or `partial + ?` with its holes; the gate's policy,
`Required`, decides what passes: `Proven` (product images) fails
unless every bound is known, `Partial` (diagnostic image classes, whose
observers the product does not carry) also passes a `partial + ?` hart with a
warning. Either way the bound, or the proven part, must fit the usable stack
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
Independently of the installed interrupt handlers, it also follows linked
compiler panic entry points, the platform panic handler and the optional
image panic hook. A resolved callee in cached memory fails the build under
both stack policies, so a panic helper or hook cannot silently move to PSRAM.
The hook and its callees also must not statically reference cached memory:
the retained ELF relocations reveal globals, lookup tables and anonymous
constants introduced by the compiler, including constant-copy initialization.

The image compiler flags have one owner, `image::configure` of
[`oer-toolchain`](../../toolchain/README.md), which a stack policy parameterizes
through `StackPolicy::image_compiler`:
`-Z emit-stack-sizes`, `-Z move-size-limit` (the policy's `max_move_bytes`) with `-D large-assignments`,
`-Z share-generics=y` and the C/C++ `-fstack-size-section`. No
`.cargo/config.toml` of the repository carries Rust flags (Cargo reads those
from the directory it runs in, not from the manifest's, so image builds from
the repository root would never see them). It sets `RUSTC_BOOTSTRAP=1` on the
image's Cargo command alone, only for those `-Z` flags on the exact stable
toolchain `rust-toolchain.toml` pins. Images build without frame pointers;
`esp-backtrace` then reports a panic's message and location without a
frame-pointer walk.

The stage-two header, checksum and address map come from the
[platform layout](../../../platform/espressif/staged-layout/README.md), which application
build scripts also use to configure the linker.

The staged runtime's placement audit (`staged::placement`) checks the
PSRAM/SRAM placement contract of the
[platform layout](../../../platform/espressif/staged-layout/README.md) and that each
PSRAM trap and interrupt entry first swaps to its SRAM stack; the caller's
audit runs beside it.

A stack policy that names no stacks sets only the move limit: the image is
compiled with every image flag and no stack gate runs. Staged S31 and C5
images both name their stacks and pin their ROM inputs. C5 keeps the
4096-byte move limit; its console pins the large serving and watchdog
futures separately before joining their references, avoiding a second move
of the console state.

## Comparing images

`compare::compare_elf` compares two linked RISC-V images function by
function modulo placement, over the `oer-elf` symbols and the decoded
listings of `oer_riscv_lift::listing`: every address an instruction forms
becomes symbol+offset, legacy mangling hashes and LLVM clone numbers are
dropped, identical-code-folded names pair by body, and a `Review` applies
reviewed aliases and scheduling ties. `cargo fw compare` calls it;
`compare images` builds both sides first (`oer_hil_image::compare_images`).

## Source inputs and exclusion

`source-inputs.json` lists the compiled sources (dep-info and build-script
inputs), the workspaces' manifests and locks, Cargo configuration, the
policies, partition tables and chip profile, and the builder's own sources:
the dependency closure of `oer-image`, `oer-image-linker` and the caller's
builder packages, as `oer-repo` resolves it, so an analyzer or linker crate
is an input from the day it is linked.

Build exclusion has one mechanism, the foundation's file lock
(`oer_process::lock::FileLock`, shared or exclusive): one build at a
time in the shared compile cache (`exclusion::compile_cache`, `<cache>/build.lock`,
waiting, held from the Cargo run until the ELF is copied out), one
build per private lock copy (`BuildLock`, refusing), the host's image build
slots (`exclusion::slot` under `host_build_root()/tokens`: half the cores, at
2 GB each), and the ESP-IDF cache (exclusive while its tree and tools change,
then converted to shared for the builds).

Run host regressions with `cargo test -p oer-image`.
