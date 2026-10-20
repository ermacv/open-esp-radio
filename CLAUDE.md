# open-esp-radio

Rust 2024 `no_std` radio stack for Espressif chips (ESP32-S31 and ESP32-C5):
Wi-Fi, Bluetooth LE and IEEE 802.15.4,
without vendor radio binaries. Decisions are in
[docs/decisions.md](docs/decisions.md), the system in
[docs/architecture.md](docs/architecture.md).

## Rules

- **Ask, never fall back.** When a choice changes a shared crate, drops data,
  adds a fallback, leaves work unfinished, changes memory use or on-hardware
  behavior, or departs from a contract shape already accepted, explain it,
  recommend an option and ask before writing code. Never silently pick a
  conservative reading, an older behavior or an approximation. Decide small,
  local, reversible details (a name, a variant, a test shape) yourself and
  state the choice in one line of the PR; batch the real questions.
- **Keep your word.** Never drop silently what you said you would keep; a
  departure from the plan you stated goes into the PR as your own decision.
  Before calling something unused, grep its field and method accesses, not
  only its type name.
- **No compatibility layers.** Change an interface together with all its
  callers: no deprecated aliases, re-export shims or fallback paths.
- **Tests.** Every behavioral change gets a focused regression test beside its
  module (`#[cfg(test)]`) or in the crate's `tests/`. Tests check behavior and
  ownership, never generated register addresses, masks, shifts or layout
  constants; time in tests comes from the virtual clocks of `oer-time-virtual`.
- **Hardware access.** MMIO only through typed PAC accessors. A missing field
  is added to `chips/<chip>/registers/` and regenerated (`cargo registers generate`), never
  written as a local mask or shift. `unsafe` stays narrow and carries
  `// SAFETY:` ([docs/unsafe.md](docs/unsafe.md)).
- **Execution.** An interrupt handler never shares a lock with a task. Driver
  code never blocks or spins; it yields to its executor. The one exception is
  a named settle of up to 20 µs ([docs/decisions.md](docs/decisions.md)).
- **Panics.** Input from outside never panics: frames from the air, any peer
  value (counters, PNs, sequence numbers, lengths, indices), host and API
  input return a handled error. Elsewhere, during development, `todo!`,
  `unreachable!`, `panic!` and `expect` (never a bare `unwrap()`) are allowed,
  each with a static message. Panic lints warn; a crate moves to `deny` when
  its contract freezes ([docs/decisions.md](docs/decisions.md)).
- **Generated files** (`*/pac-raw/src/lib.rs`, `*/pac/src/generated.rs`,
  `registers/**/published/*`, `vendor/facts/**`, all below `chips/`) are
  never read whole or edited by hand: grep them by path, regenerate them.
  `chips/*/layout/linker/dma-window.x` is small and may be read, but it is
  regenerated, never edited.
- **Sources.** No vendor binaries, disassembly or credentials in git. A
  recovered value carries its `SOURCE:`. From sources without a license only
  facts, never code or verbatim tables
  ([docs/source-policy.md](docs/source-policy.md)); the Bluetooth LE hardware
  engine is the one exception and keeps what it takes from `esp32s31-bt-lib`.
- **GitHub.** Act only in the owner's repositories; never write to upstream
  projects (fixes to esp-hal go to the owner's fork).
- **Decisions.** Only the owner decides. Add or change a line in
  `docs/decisions.md` only for an answer the owner stated explicitly, in the
  same change, without attribution or date.
- **Documents state the current state.** No plans, work lists, migration
  maps, history or "the owner decided" in the repository; notes and plans
  live outside it.

## Where code goes

The layout is [docs/repository-layout.md](docs/repository-layout.md).

- **Roots by what the code is about:** `protocols/` the sans-IO protocols and
  their vocabularies (radio, Wi-Fi, Bluetooth, 802.15.4, network, HIL);
  `portable/` generic support no protocol or chip owns (time, memory, trace,
  Embassy bindings, the HIL agent core); `chips/` everything Espressif:
  `espressif/` for IP shared by the chips, `<chip>/` for PAC, HAL, PHY,
  drivers, board, layout, chip knowledge and the chip's `platform/`,
  `registers/`, `notes/` and `vendor/` data; `firmware/` the chip images;
  `tools/<domain>/` everything for the developer's machine, a domain's
  libraries beside its program (`tools/hil/run`, `tools/hil/cli`).
- **Executables** live only in `firmware/` and `tools/`; a host program is
  `main.rs` and its own modules.
- **Direction:** protocols and portable crates know no chip and no tool; a
  host tool takes a chip fact from a crate that does not touch hardware and
  never links a HAL. Only names and the unsafe boundary are checked
  (`cargo test -p oer-test-layers`).
- **Names:** a library is `oer-` and its path below the root
  (`protocols/ieee80211/wire` is `oer-ieee80211-wire`); an executable keeps its
  root (`oer-firmware-apps-w1`, `oer-tool-image-cli`).
- Only `cargo fw` builds images; other programs take artifacts.
- A crate never sits inside another crate's directory. A new crate needs a
  reason: dependency direction, a license boundary, a different target or
  dependency set, or an independent consumer; otherwise it is a module.

## Work

- A branch per task; one worktree per session, reused: switch to the next
  task's branch inside it (`git switch -c <branch> origin/main`), so its
  `target/` stays warm. A new worktree starts its `target/` as a copy of a
  warm one (`cp -a --reflink=auto <warm>/target <new>/`); never share
  `CARGO_TARGET_DIR` between checkouts. Remove a worktree with `git worktree
  remove` (never `--force`) once its branch is merged.
- Preserve unrelated changes in a dirty tree. Conventional Commit subjects
  (`fix(esp32s31): …`).
- For what you changed: `cargo test -p <pkg>`, `cargo clippy -p <pkg>
  --all-targets`, `cargo fmt`. Chip code also builds for
  `riscv32imafc-unknown-none-elf`.
- Commands longer than about a minute (builds, CI watches, stand runs) run in
  the background; wait for their completion, never block the conversation.
- Judge a check only by its own exit code: never through a pipe such as
  `| tail` (redirect to a file and print `exit=$?` instead).
- Pass a list of paths or arguments as an array, not as one string variable.
- Cite commit hashes as `git` prints them after the commit, never from memory.
- The stand is shared: hardware runs hold the board's lock; never kill another
  holder, wait for it. Stand runs need no extra approval; the owner's limits
  on secrets, eFuse writes, protected boards and hub ports still apply.
- Open the pull request as a draft at the start (`gh pr create --draft`) and
  push the work to it. Pull requests run no CI: mark it ready when the work is
  done (`gh pr ready`), which squash-merges it and deletes the branch
  (`.github/workflows/merge.yml`). CI runs on main after each merge; a red
  main is fixed by whoever broke it, in the next pull request.
