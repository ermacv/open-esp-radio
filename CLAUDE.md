# open-esp-radio

Rust 2024 `no_std` radio stack for the Espressif ESP32-S31 (the ESP32-C5 code
lies in the tree outside the Cargo workspace and CI): Wi-Fi, Bluetooth LE and
IEEE 802.15.4, without vendor radio binaries.
Read [docs/architecture.md](docs/architecture.md) before any task: it is the
system these rules serve.

## Code

- **Ask, never fall back.** When a choice changes a shared crate, drops data,
  adds a fallback, leaves work unfinished, changes memory use or on-hardware
  behavior, or departs from a contract shape already accepted, explain it,
  recommend an option and ask before writing code. Decide small, local,
  reversible details (a name, a variant, a test shape) yourself and state
  each in one line of the PR; batch the real questions.
- **Keep your word.** A departure from the plan you stated goes into the PR
  as your own decision. Before calling something unused, grep its field and
  method accesses, not only its type name.
- **No compatibility layers.** Change an interface together with all its
  callers: no deprecated aliases, re-export shims or fallback paths.
- **Chip ownership.** The S31 workspace contains no esp-* platform crates
  or bridges. Host `espflash` and its transitive image-format dependency are
  host tools. The PAC's single `take()` claims the chip; the HAL's
  `Root::take` splits it through `into_bootstrap` or `into_runtime`.
  Only these owned partitions grant register access. C5 stays outside the
  workspace.
- **Compile time first.** Every check that can run at compile time runs there.
  A `const fn` alone does not enforce this: fixed values use const items or
  blocks, and generic geometry uses inline const checks at monomorphization.
- **Panics and unsafe.** A change introduces no new runtime panic sites or
  `unsafe` code. Unsafe contracts cover memory safety; operation order and
  lifecycle are enforced by `&mut` ownership and typestate. Moving existing
  unsafe sites into their narrow home (`oer-riscv` for architectural
  instructions, `oer-memory` for cells, placement and DMA proofs, the HAL's
  PSRAM owner for slice creation) may increase the home's count only by the
  moved sites, when the touched crates' total decreases and no calling crate's
  count increases; each site retains its narrow
  contract and `// SAFETY:` proof. Compile-time checks and test assertions are allowed.
  Context-local mutable payloads use `oer-memory::ContextCell` with a private
  `ContextProof`; nested access is refused without consuming the payload.
  An impossible state returns a named fault retaining its source observations;
  its owners stay quarantined, then the image calls safe `fatal::reset_recording`
  to retain the reason for the next boot to print. A watchdog is not this mechanism.
  Owned PHY transactions preserve their registration and restore obligations;
  only consuming reunion with the exact affine return owner releases their
  quarantine guard. Dropping either half records its source observations and
  resets, without automatic restoration or owner release. The HAL owns both
  signatures of its fault binding through its exported reporter macro.
  Isolated verifier probes keep their own terminal reporter and first fault
  in session RAM; they do not link the staged runtime.
  Remove existing runtime panics from image crates, including probes and
  HIL; image panic budgets only shrink to 0. `const` panics evaluated at
  compile time are allowed. Air, peer, host and API input return a handled
  error. New panic-lint `allow` and `expect` attributes are test-only. A crate
  moves each clean panic lint to crate-root `deny` in the PR that clears it,
  keeping workspace lint inheritance. `debug_assert!` is not a guard.
  Arithmetic on peer values names its overflow behavior with `checked_`,
  `wrapping_` or `saturating_` operations.
- **Execution.** The critical section is hart-local; an image confines it to
  one declared hart. An interrupt handler never shares a lock with a task. A
  task waker stays on its executor's hart; no waker crosses harts. Executor
  startup borrows the runtime's affine `Hart<H>` and const-checks its route's
  hart. Interrupt enable consumes pre-admission authority and returns the
  router with the same hart token; before enable they may be borrowed together.
  An executor's urgent sources come from the typed observations returned by
  the same constructors as its doorbells; an empty composition passes `()`.
  A signal names its raisers at compile time. A raising handler sits strictly
  above the receiving executor; the one exception is an
  `Exempt(ExecutorLevel)` handler on that executor's hart and level
  (`FromExecutorLevel`), whose signal the next round serves. Driver code
  never blocks or spins; it yields to its executor. The one exception is a
  named settle of a known length up to 20 µs. Before an executor exists
  (the bootstrap and early boot) there is nothing to yield to: console
  output there may wait for room in the USB FIFO up to a bounded budget per
  record, and hardware bring-up (PLL, RC calibration, cache, MSPI, PSRAM)
  may poll in named boot waits, each with its own bound from its source and
  all under one boot deadline below the armed RTC watchdog; a timeout is a
  typed boot failure, never a fallback value. Portable protocols take no
  `critical-section` and no blocking mutex.
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
  its `SOURCE:`.
  A typed trigger in a readable mixed word preserves ordinary fields and
  unassigned bits, writes zero to WTC and other WT lanes, and sets only its
  selected WT field. Generic modify remains unavailable for a WTC word.
  A `read-observed` field grants observation without asserting hardware
  read-only semantics. Its word has no generic writer or read-modify-write;
  only a separately sourced `zero-register-writes` operation may publish
  the complete zero word, without reading or implying an acknowledgement.
  Generated register code has no `unsafe`: volatile access lives in
  `portable/mmio` alone. `unsafe` stays in `portable/mmio`, architectural
  instructions in `portable/riscv`, the PAC's one root claim, probe images,
  the placement constructor and the HAL's protocol core, narrow and with `// SAFETY:`
  ([docs/unsafe.md](docs/unsafe.md)); crates above the HAL forbid it.
  Only `oer-riscv` contains `asm!` for architectural operations. Entry/trap
  ABI assembly uses `global_asm!` or `naked_asm!` only in staged entry/trap
  code, bootstrap and probes; host compiler fixtures are test inputs.
- **Hardware dictionaries and geometry.** Field codes and opcodes are named
  values in the register model, including analog-I2C and hardware-consumed
  memory layouts; HAL selects the generated names. Field widths, maxima,
  register-array counts and block geometry come from generated constants.
  Sequence settings remain HAL constants with `SOURCE:`. An observed image
  without an established code meaning stays cited configuration, not a model
  dictionary entry.
- **Air formats** use the `oer-wire` field macro and its bounded cursor; each
  field names its clause of the standard. No hand-written masks or shifts.
- **Tests.** Every behavioral change gets a focused regression test beside its
  module (`#[cfg(test)]`) or in the crate's `tests/`. Tests check behavior and
  ownership, never generated register addresses, masks, shifts or layout
  constants; time comes from `oer-time-virtual`. A test is hermetic and safe
  in parallel: its own memory and its own temporary directory (`tempfile`);
  no external program, process-wide state, fixed path, network port or real
  time. The one exception is the pinned toolchain (`cargo`, `rustc`, `lld`)
  run into the test's own build directory for compile-fail and
  compiler-driven cases.
- **Generated files.** `chips/*/pac/src/generated.rs`,
  `chips/*/vendor/facts/**/*.toml` (including TOML files directly in `facts/`)
  and `chips/*/vendor/phy-baselines/*/*.json` are never read whole or edited
  by hand: grep them by path, regenerate them. `cargo registers generate
  --chip <chip>` renders the PAC and
  `chips/*/layout/linker/dma-window.x`; the linker fragment may be read,
  never edited. Its inputs are `chips/<chip>/registers/chip.toml`, `types/`,
  `layouts/`, `evidence/` and the schema-6 `policy.toml`.
- **Sources.** No vendor binaries, disassembly or credentials in git. A
  recovered value carries its `SOURCE:`. From sources without a license only
  facts, never code or verbatim tables
  ([docs/source-policy.md](docs/source-policy.md)); the Bluetooth LE hardware
  engine is the one exception. To find something in vendor libraries, ROM or
  a vendor image, use Blobray (`cargo blobray`,
  [tools/analysis/blobray-README.md](tools/analysis/blobray-README.md)) first.
- **Where code goes** ([docs/repository-layout.md](docs/repository-layout.md)):
  `protocols/` sans-IO protocols, `portable/` generic support, `chips/`
  everything Espressif, `firmware/` chip images, `tools/<domain>/` the
  developer's machine. Executables only in `firmware/` and `tools/`. A library
  is `oer-` plus its path below the root; an executable keeps its root
  (`oer-firmware-apps-w1`). A crate never sits inside another; a new crate
  needs a reason (dependency direction, license, target, independent
  consumer). Only `cargo fw` builds images. Every package builds.

## Work

- **Do what the task asks and check what it names.** No stand run,
  comparison or measurement series the task does not ask for. A question the
  task does not answer goes to whoever set it.
- **Evidence is short.** A pull request lists the checks it ran with their
  exit codes, run IDs and a short table. Longer text goes to the notes
  repository outside this one, unhashed. A binary kept outside git (a vendor
  image or capture, a raw dump) goes to
  `<XDG data>/open-esp-radio/evidence/<task>/` with one SHA-256 line in the
  notes; nothing else is hashed or inventoried. `target/` holds only output
  that can be rebuilt.
- **Checks on the host:** `cargo test -p <pkg>`, `cargo clippy -p <pkg>
  --all-targets` and `cargo fmt --check` for what you changed, plus `cargo
  test -p oer-test-layers`. Images, image checks and sizes are built by CI.
  Changed chip packages may also run `cargo check` and `cargo clippy` with
  `--target riscv32imafc-unknown-none-elf -p <pkg>` as type-checks only,
  without images or linking. One image build is allowed only to settle a
  concrete doubt, named in the PR. Judge a check only by its own exit code,
  never through a pipe (redirect to a file and print `exit=$?`).
- **Sizes.** CI reports memory growth (code, `.rodata`, SRAM and PSRAM
  sections, a type, future or frame, an allocated task or interrupt stack);
  growth is not a stop. A limit the owner sets for an image is its own gate.
- **Stand.** Runs only when they answer a real question. Hardware runs hold
  the board's lock; never kill another holder, wait. The owner's limits on
  secrets, eFuse writes, protected boards and hub ports apply. A claim about
  what the hardware puts on air is used only after a stand run confirms it.
- **Branches and worktrees.** A branch per task; one worktree per session,
  reused (`git switch -c <branch> origin/main`). A new worktree copies a warm
  `target/` (`cp -a --reflink=auto`); never share `CARGO_TARGET_DIR`. Scratch
  stays under the clone's `target/`, never in `/tmp`. Remove a merged
  worktree with `git worktree remove` (never `--force`). Preserve unrelated
  changes in a dirty tree.
- **Long commands** (builds, CI watches, stand runs) run in the background;
  never block the conversation.
- **Pull requests.** Conventional Commit subjects (`fix(esp32s31): …`). Open
  the PR as a draft at the start, mark it ready when done (`gh pr ready`).
  The owner merges; an agent never merges. CI builds everything; nobody waits
  for it: a PR is ready when its host checks pass. A red CI is fixed by
  whoever broke it. Rebase only
  on a conflict or when main changed the same files. Cite commit hashes as
  `git` prints them. Pass lists of paths as arrays, not one string.
- **GitHub.** Act only in the owner's repositories; upstream projects are
  read-only.
- **Documents state the current state,** in English. No plans, history or
  "the owner decided" in the repository. An owner decision changes this file,
  [docs/architecture.md](docs/architecture.md) or the README of the code it
  governs, in the same change.
