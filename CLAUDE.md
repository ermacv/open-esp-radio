# open-esp-radio

Rust 2024 `no_std` radio stack for the ESP32-S31: Wi-Fi, Bluetooth LE and
IEEE 802.15.4, without vendor radio binaries. Owner decisions are in
[docs/decisions.md](docs/decisions.md), the system in
[docs/architecture.md](docs/architecture.md).

## Rules

- **Ask, never fall back.** When a choice changes a shared crate, drops data,
  adds a fallback, leaves work unfinished or changes on-hardware behavior,
  explain it, recommend an option and ask the owner. Never silently pick a
  conservative reading, an older behavior or an approximation.
- **No compatibility layers.** Change an interface together with all its
  callers: no deprecated aliases, re-export shims or fallback paths.
- **Tests.** Every behavioral change gets a focused regression test beside its
  module (`#[cfg(test)]`) or in the crate's `tests/`.
- **Hardware access.** MMIO only through typed PAC accessors. A missing field
  is added to `registers/` and regenerated (`cargo registers generate`), never
  written as a local mask or shift. `unsafe` stays narrow and carries
  `// SAFETY:` ([crates/UNSAFE.md](crates/UNSAFE.md)).
- **Execution.** An interrupt handler never shares a lock with a task. Driver
  code never blocks or spins; it yields to its executor.
- **Panics.** Input from outside never panics: frames from the air, any peer
  value (counters, PNs, sequence numbers, lengths, indices), host and API
  input return a handled error. Elsewhere, during development, `todo!`,
  `unreachable!`, `panic!` and `expect` (never a bare `unwrap()`) are allowed,
  each with a static message. Panic lints warn; a crate moves to `deny` when
  its contract freezes ([docs/decisions.md](docs/decisions.md)).
- **Generated files** (`*/pac-raw/src/lib.rs`, `*/pac/src/generated.rs`,
  `registers/*/published/*`, `knowledge/*/vendor/facts/**`) are never read
  whole or edited by hand: grep them by path, regenerate them.
- **Sources.** No vendor binaries, disassembly or credentials in git. A
  recovered value carries its `SOURCE:`. From sources without a license only
  facts, never code or verbatim tables
  ([docs/source-policy.md](docs/source-policy.md)).
- **GitHub.** Act only in the owner's repositories; never write to upstream
  projects (fixes to esp-hal go to the owner's fork).
- **Decisions.** Only the owner decides. Add a line to `docs/decisions.md` only
  for an answer the owner stated explicitly, in the same change.

## Where code goes

Ask who knows the hardware: ESP32-S31 registers go to `crates/esp32s31/`,
Espressif IP and vendor algorithms shared by their chips to
`crates/espressif/`, portable protocol code to its protocol directory, and
code without chip or protocol to `crates/base/`, `crates/radio/` or
`crates/exec/`. Bindings to external frameworks live in `crates/adapters/`.
A crate never sits inside another crate's directory. A new crate needs a
reason: dependency direction, a license boundary, a different target or
dependency set, or an independent consumer; otherwise it is a module.

| Path | Holds |
| --- | --- |
| `crates/base/` | time, virtual time, memory, network values, deadline |
| `crates/exec/` | interrupt table, Embassy executor, esp-hal interrupt binding |
| `crates/radio/` | the radio port lifecycle, permit pool, coexistence vocabulary |
| `crates/ieee80211/`, `bluetooth/`, `ieee802154/` | portable protocols |
| `crates/espressif/` | Espressif PACs, coexistence, 802.15.4 engine, analog bus, modem clock, Wi-Fi policy data |
| `crates/esp32s31/` | PAC, HAL, PHY, Wi-Fi driver, radio coexistence |
| `crates/adapters/` | Embassy time and network, esp-hal SoC |
| `firmware/` | the chip-target workspace: staged boot platform, images |
| `registers/`, `tools/registers/` | the register model and its SVD/PAC generator |
| `knowledge/` | recovered facts and research notes (data, not code) |

## Work

- One branch and worktree per task, each with its own `target/`; never share
  `CARGO_TARGET_DIR` between checkouts. Conventional Commit subjects
  (`fix(esp32s31): …`).
- For what you changed: `cargo test -p <pkg>`, `cargo clippy -p <pkg>
  --all-targets`, `cargo fmt`. Chip code also builds for
  `riscv32imafc-unknown-none-elf`.
- `git push` the branch and open a draft PR (`gh pr create --draft`); the owner
  reviews and merges.
