# open-esp-radio

Rust 2024 `no_std` radio stack for Espressif chips, today the ESP32-S31:
Wi-Fi, Bluetooth LE and IEEE 802.15.4, without vendor radio binaries.
Read [docs/architecture.md](docs/architecture.md) before any task: it is the
system these rules serve. This file holds the rules; the contracts of each
subsystem live in its page or README, listed under **Subsystem contracts**.

## Code

- **Ask, never fall back.** When a choice changes a shared crate, drops data,
  adds a fallback, leaves work unfinished, changes on-hardware
  behavior, or departs from a contract shape already accepted, explain it,
  recommend an option and ask before writing code. Decide small, local,
  reversible details (a name, a variant, a test shape) yourself and state
  each in one line of the PR; batch the real questions. Memory changes are
  reported in the PR, not approval gates; shared-contract changes needed to
  return typed errors follow the same rule, with every caller migrated.
- **Keep your word.** A departure from the plan you stated goes into the PR
  as your own decision. Before calling something unused, grep its field and
  method accesses, not only its type name.
- **No compatibility layers.** Change an interface together with all its
  callers: no deprecated aliases, re-export shims or fallback paths.
- **Chip ownership.** The S31 workspace contains no esp-* platform crates
  or bridges. Host `espflash` and its transitive image-format dependency are
  host tools. The PAC's single `take()` claims the chip; the HAL's
  `Root::take` splits it through `into_bootstrap` or `into_runtime`.
  Only these owned partitions grant register access.
- **Compile time first.** Every check that can run at compile time runs there;
  a runtime check stands only where that proof is impossible, and their number
  tends to zero. A runtime check answers what no build can know: the
  hardware, the air, a peer, the host, the application's input, time and
  load. What this repository's own code decides is not among them: a place
  is handed out once by its owner's claim, never asked for and refused; a
  checked value made from constants is built in a const item, by the
  constructor that refuses the same value given at run time; a contract
  between two of our modules is a type or a proof, never a violation
  variant; a store with a slot for every token that can enter it cannot
  refuse. A `const fn` alone does not enforce this: fixed values use
  const items or blocks, and generic geometry uses inline const checks at
  monomorphization. An existing runtime check of these kinds is debt to
  remove, never a precedent to repeat.
- **Errors stay errors.** An error never becomes a status, a dummy value or a
  success. A refusal for temporary unavailability exists only where it is
  needed and proven, and it names what comes next: a release event, a
  registered waker, a retry time, or a final refusal returning its owners. A
  permanent fault is never reported as temporary. A status may accompany a
  result or keep a diagnostic such as the last fault; it never replaces a
  result. Every API names its acceptance point: before it a refusal, after it
  an outcome or a fault. A confirmed result never becomes unknown through a
  later failure, and a side effect of unknown result is not repeated
  automatically. Air, peer, host and API input return a handled error.
  [Operation results](docs/architecture.md#operation-results) is the
  decision procedure.
- **Decisions are values.** A function another module calls to learn
  whether a transition may happen never answers `bool` or a bare variant:
  it returns the proof of the transition, or the value that names
  everything blocking it, and it changes nothing while it answers. A
  predicate of one fact inside its own module may be `bool`.
- **State stays in its place.** An object with state larger than two
  machine words (a machine, a driver, a table, a queue, an owner of memory)
  is built where it will live and reached by reference from then on: no
  constructor returns it to be moved into its place, no transition consumes
  it, no function passes it on by value. Ownership crosses a function, a
  task or a hart as at most two machine words: a small `Copy` value, a
  proof, or the handle of the place the object stays in. A refusal leaves
  its owner where it was: the callee is lent the place and takes the owner
  at its acceptance point. Outside the rule are data kept between turns (a
  payload, a stored block), whose copy names its reason at the site, and a
  report made once an operation (a failure with its diagnostic path). An
  existing move of state is debt to remove, never a precedent to repeat.
  The measure is the image's copy and frame reports
  ([image checks](tools/image/checks/README.md)), not the source's shape:
  the compiler elides no move by contract.
- **Runtime panics.** Runtime code has zero panics; a change adds no runtime
  panic site. Compile-time panics and test assertions are allowed; a
  compile-time check is `const { if !cond { panic!("…") } }` as a const item
  or inline const, never an assertion macro. `debug_assert!` is not a guard.
  Arithmetic on peer values names its overflow behavior with `checked_`,
  `wrapping_` or `saturating_` operations. A removed panic leaves its owners
  kept, the operation's outcome correct and progress assured; the panic count
  is a secondary measure. Remove existing runtime panics from image crates,
  probes and HIL included; image panic budgets report counts against a
  target of 0 and are not build gates.
- **Sans-IO crates** (the layer in `docs/architecture.md`) start with
  `#![forbid(unsafe_code)]` and contain no runtime panic paths: `panic!`,
  `unreachable!`, `todo!`, `unimplemented!`, `assert*`, `debug_assert*`,
  `vendor_assert!`, `unwrap`/`expect`, unchecked indexing or slicing,
  overflow, invalid division and panicking core methods return typed errors
  instead. They never depend on fatal, reset or retained-record owners and
  never reset.
- **Panic lints.** New panic-lint `allow` and `expect` attributes are
  test-only. A crate moves each clean panic lint to crate-root `deny` in the
  PR that clears it, keeping workspace lint inheritance. Workspace
  `disallowed_methods` stays `deny`; `disallowed_macros` keeps its default
  level. Every local Clippy configuration retains all common method bans; a
  cleaned sans-IO crate's `clippy.toml` holds the union described in
  [docs/unsafe.md](docs/unsafe.md) before it denies both lints at its root.
- **Terminal decisions.** A reset or a runtime panic exists only at the
  hardware level, behind a proof marked `HW_PANIC_SOURCE: <name>` at the site
  and named in `[proofs]` of `tools/test/layers/terminal-sites.toml`. The
  proof is a statement that the state is impossible, naming the model,
  revision, preconditions, event order and limits (a cited value alone proves
  no lifecycle invariant), or an observed hardware failure where continuing
  would hand another owner memory or a peripheral the hardware may still use.
  Without such a proof, return a handled typed error and handle it at its own
  level: an error never travels up the stack to end in a panic or reset. An
  impossible state returns a named fault retaining its source observations;
  its owners stay quarantined, then the image or HAL owner calls safe
  `fatal::reset_recording`, without `panic!`. A watchdog is not this
  mechanism. That sequence is no license to quarantine. A quarantine (a
  peripheral, an installation, an owner or a buffer left unusable until reset:
  a poisoned state, memory kept out of its pool on purpose) counts as a
  runtime panic: every peripheral and every owner has a recovery path. A new
  one needs a proof that no handled error can keep the owners, and the owner's
  approval before any code is written. Existing quarantine, in the PHY and in
  the Wi-Fi path, is debt to remove, never a precedent to repeat. The
  subsystem contracts linked from this file do not lift this requirement. A
  software contract violation not yet removed by types restarts the station
  and records its cause, never the chip. The policy and its limits (debugger,
  retained diagnostics, repeated boot failure) are in [Fatal decisions and
  reset](docs/architecture.md#fatal-decisions-and-reset).
- **Unsafe.** A change introduces no new `unsafe` code outside the homes
  below; inside a home, new `unsafe` needs its benefit shown in the PR (what
  it replaces and what it costs) and the architect's approval. Unsafe
  contracts cover memory safety; operation order and lifecycle are enforced by
  `&mut` ownership and typestate. `unsafe` stays in `portable/mmio` (volatile
  access; generated register code has none), `portable/riscv` (architectural
  instructions), `portable/memory` (cells, placement, DMA proofs, typed symbol
  exports), `portable/waker`, Espressif interrupt bindings in
  `chips/family/platform/interrupt-table`, the PAC's one root claim, probe
  images, the placement constructor, the HAL's PSRAM owner (slice creation)
  and the HAL's protocol core, narrow and with `// SAFETY:`; crates above the
  HAL forbid it. Moving an existing site into its narrow home may raise the
  home's count only by the moved sites, when the touched crates' total falls
  and no calling crate's count grows; each site retains its narrow contract
  and `// SAFETY:` proof. The permitted exceptions are listed in
  [docs/unsafe.md](docs/unsafe.md). Only `oer-portable-riscv` contains `asm!`
  for architectural operations. Entry/trap ABI assembly uses `global_asm!` or
  `naked_asm!` only in staged entry/trap code, bootstrap and probes; host
  compiler fixtures are test inputs.
- **Execution.** The critical section is hart-local. An interrupt handler
  never shares a lock with a task. A task waker stays on its executor's
  hart; no waker crosses harts. A signal names its raisers at compile time;
  a raising handler sits strictly above the receiving executor, with the
  exceptions listed in [Execution](docs/architecture.md#execution). Driver
  code never blocks or spins; it yields to its executor. The one exception
  is a named settle of a known length up to 20 µs. Before an executor exists
  (the bootstrap and early boot), console output may wait for room in the
  USB FIFO up to a bounded budget per record, and hardware bring-up (PLL,
  RC calibration, cache, MSPI, PSRAM) may poll in named boot waits, each
  with its own bound from its source and all under one boot deadline below
  the armed RTC watchdog; a timeout is a typed boot failure, never a
  fallback value. Portable protocols take no `critical-section` and no
  blocking mutex.
- **Static call graph.** Library code an image's roots reach (drivers,
  protocols, runtimes, the platform, the application entry) makes no
  indirect call but through the executor's one waker vtable, the generated
  interrupt table and a compiler jump table, and never reaches `core::fmt`:
  no trait object, no function pointer chosen at run time, no `write!`,
  `format_args!` or `Debug`/`Display` call outside tests and host tools. A
  `dyn` the optimizer happens to devirtualize is still one. A closed set is
  a generic parameter, an enum or a tuple; diagnostics are binary records
  the host renders (`oer-portable-diagnostics`). The image stack analysis
  bounds a stack only over such code
  ([stack bounds](tools/image/stack/README.md)): an image that declares
  `task-stack = "proven"` does not link otherwise, and every interrupt
  handler follows the rule in every image. An application may use both in
  its own tasks by declaring its task stack's size in place of the proof.
- **Hardware facts.** A fact about the hardware (an address, a memory
  region, a peripheral, an interrupt source, a CPU line count, the number of
  cores, an ISA extension, a ROM function) has one source of truth in the
  chip facts and the register model, each value with its `SOURCE:`. Code,
  linker scripts and tables where a person could err (vector tables, the
  binding of a line to its source and handler, memory layout, `--defsym`,
  stack placement) are generated from that source; only what the facts
  cannot express is written by hand.
- **Hardware access.** MMIO only through typed PAC accessors. A missing field
  is added to `chips/<chip>/registers/` and regenerated (`cargo registers
  generate --chip <chip>`), never written as a local mask or shift. Code
  preserves reserved and unknown bits; the one exception is a PAC policy
  fixed image marked `whole-word`, a vendor's store of a whole word with
  its `SOURCE:`. Write-trigger (WT/WTC) and `read-observed` words follow
  [tools/registers/model/README.md](tools/registers/model/README.md).
- **Hardware dictionaries and geometry.** Field codes and opcodes are named
  values in the register model, including analog-I2C and hardware-consumed
  memory layouts; HAL selects the generated names. Field widths, maxima,
  register-array counts and block geometry come from generated constants.
  Sequence settings remain HAL constants with `SOURCE:`. An observed image
  without an established code meaning stays cited configuration, not a model
  dictionary entry.
- **Air formats** use the `oer-portable-wire` field macro and its bounded
  cursor; each field names its clause of the standard. No hand-written masks
  or shifts.
- **Tests.** Every behavioral change gets a focused regression test beside its
  module (`#[cfg(test)]`) or in the crate's `tests/`. Tests check behavior and
  ownership, never generated register addresses, masks, shifts or layout
  constants; time comes from `oer-portable-time-virtual`. A test is hermetic
  and safe in parallel: its own memory and its own temporary directory
  (`tempfile`); no external program, process-wide state, fixed path, network
  port or real time. The exceptions are the pinned toolchain (`cargo`,
  `rustc`, `lld`) run into the test's own build directory for compile-fail
  and compiler-driven cases, and the vendor UDP receiver's link host test,
  which builds with a host C11 compiler as
  [hil/scenarios/wifi-load/README.md](hil/scenarios/wifi-load/README.md)
  describes.
- **Generated files.** `chips/*/pac/src/generated.rs` and
  `chips/*/vendor/facts/**/*.toml` (including TOML files directly in
  `facts/`) are never read whole or edited by hand: grep them by path,
  regenerate them. `cargo registers generate --chip <chip>` renders the PAC
  and `chips/*/platform/layout/linker/dma-window.x`; the linker fragment may
  be read, never edited. Its inputs are `chips/<chip>/registers/chip.toml`,
  `types/`, `layouts/`, `evidence/` and the schema-6 `policy.toml`.
- **Sources.** No vendor binaries, disassembly or credentials in git. A
  recovered value carries its `SOURCE:`. From sources without a license only
  facts, never code or verbatim tables
  ([docs/source-policy.md](docs/source-policy.md)); the Bluetooth LE hardware
  engine is the one exception. To find something in vendor libraries, ROM or
  a vendor image, use Blobray (`cargo blobray`,
  [tools/analysis/blobray-README.md](tools/analysis/blobray-README.md)) first.
- **Where code goes** ([docs/repository-layout.md](docs/repository-layout.md)):
  `tools/<domain>/` the developer's machine, `portable/` generic `no_std`
  support, `protocols/` sans-IO protocols, `chips/` the hardware layers
  (`chips/family` what every chip of the family shares, `chips/<chip>` one
  chip, `chips/<chip>/app` its application entry), `oer/` the application
  entry an image uses, `examples/` examples on it, `hil/` checks on hardware
  (`agent`, `services`, `images` on the entry, `probes` below it,
  `scenarios` their host programs). Executables only in `examples/`, `hil/`,
  `tools/` and a chip's `bootstrap`. Every package is `oer-` plus its whole
  path (`oer-chips-esp32s31-hal`, `oer-hil-probes-esp32s31-mcycle`); the
  entry is `oer`. A crate never sits inside another; a new crate needs a
  reason (dependency direction, license, target, independent consumer).
  Only the image pipeline builds images: `cargo fw`, and the HIL harness for
  a scenario's image; an image is named by its directory (a HIL image as
  `hil-<directory>`). An example or HIL image names no chip: it depends on
  `oer` with the capabilities it needs (`wifi`, `console`, `group`, …), and
  the pipeline selects the board's chip and links the image for it. Every
  package builds.
- **Names** of standard mechanisms follow the standard that defines them
  ([Names of standard mechanisms](docs/architecture.md#names-of-standard-mechanisms)).
- **HIL.** The HIL core (`protocols/hil`, `tools/hil/link`) only carries
  data and commands between the host and the device under test; it stores
  nothing and knows no scenario, service or stand equipment. A scenario's
  host program holds its cases and verdict; the image it runs serves
  services whose every endpoint has a handler (`oer_protocols_hil_agent::service!`);
  several scenarios share one image. A HIL image is built on the product API
  (`oer`) and takes its runs' control at runtime from the host. Stand
  equipment, its drivers and its ESP-IDF peer firmware belong to `tools/lab`.

## Subsystem contracts

A change to one of these follows its page; this file does not repeat them.
A page never relaxes a rule above: where they differ, the rule above holds.

| Subject | Contract |
|---|---|
| Unsafe homes and their exceptions (PHY-fault `LinkSlot`, trace clock slot, `ContextCell`), lint catalogues, panic budgets and `panic_const_async_fn_resumed` | [docs/unsafe.md](docs/unsafe.md) |
| Harts, executors, interrupt enable, signal raisers, pilot handlers and lines | [Execution](docs/architecture.md#execution), [interrupt-table](chips/family/platform/interrupt-table/README.md), [executor](chips/family/platform/executor/README.md) |
| Fatal policy, terminal-site classes and reset owners | [Fatal decisions and reset](docs/architecture.md#fatal-decisions-and-reset), [terminal-site registry](tools/test/layers/README.md#terminal-site-registry) |
| IEEE 802.15.4 pre-bind discard, runtime acknowledgements, secured ACK | [docs/unsafe.md](docs/unsafe.md), [engine](chips/family/radio/ieee802154/engine/README.md) |
| BLE terminal output release, owned PHY transactions and their quarantine | [docs/unsafe.md](docs/unsafe.md), [HAL](chips/esp32s31/hal/README.md), [PHY](chips/family/radio/phy/README.md) |
| WT/WTC triggers, `read-observed` and `zero-register-writes` words | [register model](tools/registers/model/README.md) |
| Runtime-switchable network-core diagnostics, the one exception to zero-cost instrumentation | [Switchable network-core diagnostics](docs/architecture.md#switchable-network-core-diagnostics) |

## Work

- **Roles.** The architect, an agent the owner appoints, sets tasks as files
  in the notes repository (`tasks/`), reviews every result against the code
  and merges. An executor implements, runs the long and stand runs and reports
  in the form its task names. A reported check the architect cannot repeat is
  not evidence.
- **Do what the task asks and check what it names.** No stand run,
  comparison or measurement series the task does not ask for. A question the
  task does not answer goes to whoever set it.
- **Ownership and shutdown contracts.** These rules govern every change to such
  a contract; a local fix needs no table for form's sake.
  - **State the property first.** Before changing an ownership or shutdown
    contract, name its acceptance point, owners, obligations and terminal
    outcomes. State the assumptions and the cases outside the change.
  - **Trace every transition and consumer.** List creation, transfer,
    cancellation, return, destruction, retirement and reuse, together with every
    reader and policy decision. When ownership moves, inspect consumers that did
    not change too.
  - **Check the public contract independently.** Exercise adverse event orders
    and lifecycle boundaries through the public API. Keep expected ownership and
    results separate from the implementation being checked. Test counts and
    mutations cover the scenarios written; they do not establish missing
    scenarios.
  - **Review before integration.** Such a change needs independent review of the
    exact revision before merge and before dependent work builds on it. Record
    unresolved findings. After a revision changes, recheck the affected property;
    previous review does not automatically cover the new revision.
  - **Report what the evidence establishes.** Distinguish observed results, type
    guarantees, assumptions and untested boundaries. Do not turn passing checks
    into a broader guarantee or close a milestone while its agreed acceptance
    conditions remain open.
- **Evidence is short.** A pull request lists the checks it ran with their
  exit codes, run IDs and a short table. Longer text goes to the notes
  repository outside this one, unhashed. A binary kept outside git (a vendor
  image or capture, a raw dump) goes to
  `<XDG data>/open-esp-radio/evidence/<task>/` with one SHA-256 line in the
  notes; nothing else is hashed or inventoried. `target/` holds only output
  that can be rebuilt.
- **Checks on the host:** `cargo test -p <pkg>`, `cargo clippy -p <pkg>
  --all-targets` and `cargo fmt --check` for what you changed, plus `cargo
  test -p oer-tools-test-layers`. Images, image checks and sizes are built by
  CI. Changed chip packages may also run `cargo check` and `cargo clippy`
  with `--target riscv32imafc-unknown-none-elf -p <pkg>` as type-checks
  only, without images or linking. Judge a check only by its own exit code,
  never through a pipe (redirect to a file and print `exit=$?`).
- **Image builds.** A targeted image build measures one concrete change in
  one image, with its reason and expected result recorded in the PR before
  the build. First inspect captured ELF/type evidence. Image builds never
  replace CI or survey other images/features; other images' results come
  from the PR's CI. If three consecutive builds do not improve the target
  metric, stop and report the reason and question.
- **Sizes.** CI reports memory growth (code, `.rodata`, SRAM and PSRAM
  sections, a type, future or frame, an allocated task or interrupt stack);
  growth is not a stop. Stack guards are reported, never a gate; the proven
  IRQ bound plus margin must fit the contract maximum without guard bytes.
  A physical IRQ guard is omitted when it does not fit beside that bound and
  margin. A limit the owner sets for an image is its own gate.
- **Stand.** Runs only when they answer a real question. Hardware runs hold
  the board's lease from the LAB service; never preempt another holder,
  wait. The owner's limits on secrets, eFuse writes, protected boards and hub
  ports apply. A claim about what the hardware puts on air is used only after
  a stand run confirms it.
- **Branches and worktrees.** A branch per task; one worktree per session,
  reused (`git switch -c <branch> origin/main`). Never share
  `CARGO_TARGET_DIR`. Scratch stays under the clone's `target/`, never in
  `/tmp`. Remove a merged worktree with `git worktree remove` (never
  `--force`). Preserve unrelated changes in a dirty tree.
- **Long commands** (builds, CI watches, stand runs) run in the background;
  never block the conversation.
- **Pull requests.** Conventional Commit subjects (`fix(esp32s31): …`). Open
  the PR as a draft at the start, mark it ready when done (`gh pr ready`). The
  owner or the architect merges, after reviewing the exact revision and
  repeating its host checks; an executor never merges. CI builds everything;
  nobody waits for it, a merge included: a PR is ready when its host checks
  pass. A red CI is fixed by whoever broke it. Rebase only on a conflict or
  when main changed the same files. Cite commit hashes as `git` prints them.
  Pass lists of paths as arrays, not one string.
- **GitHub.** Act only in the owner's repositories; upstream projects are
  read-only.
- **Documents state the current state,** in English. No plans, history or
  "the owner decided" in the repository. An owner decision changes this file,
  [docs/architecture.md](docs/architecture.md) or the README of the code it
  governs, in the same change.
