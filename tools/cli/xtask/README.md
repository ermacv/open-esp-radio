# Repository commands

`oer-xtask` (`cargo xtask`) holds the repository commands a person runs by
hand to build or test: API documentation, lock files, worktrees, push, and
the compile-fail and feature-set tests. It runs no gate, no CI planning and
no review. Domain work lives with its owners and their command lines:
images in [`oer-image`](../image/pipeline/README.md) and
[`oer-hil-image`](../../hil/host/README.md), vendor probes, scenario runs and
shard regeneration in [`oer-vendor-evidence`](../../verification/evidence/README.md)
(`cargo verification`), the register inventory in the
[register tool](../registers/README.md).

| Command | Contract |
| --- | --- |
| `cargo xtask doc` | Build API documentation from each production package's `[package.metadata.docs.rs]` (its target and features) with `RUSTDOCFLAGS=-D warnings`, then run the host doctests |
| `cargo xtask check ui` | Compile every case in `tests/ui.toml` and its positive twin, checking the exact rustc diagnostics ([contract diagnostics](#contract-diagnostics)) |
| `cargo xtask check feature-sets` | Test every root-workspace package with each feature set its `open-radio.test-feature-sets` declares |
| `cargo xtask lock [--check]` | Update every workspace's `Cargo.lock` to its manifests after a dependency or pin change (online); `--check` only verifies, offline, that every lock matches its manifests and names every stale one. `cargo tidy fetch` downloads what the locks name and the cache lacks: its independent host-only workspace needs no firmware Git sources to start. If tidy's registry dependencies are missing, use `cargo --config net.offline=false tidy fetch` to bootstrap it |
| `cargo xtask worktree add PATH --branch B [--from REV]` / `remove PATH` / `prepare` | Create a Git worktree on a new branch from `REV` (default `origin/main`, fetched first when it names an `origin/` branch, so the branch starts on the current `main`) whose `target/` starts from this checkout's build outputs (without incremental data, HIL outputs and vendor firmware builds, whose CMake caches name the source checkout), so only the workspace's own crates rebuild; `remove` deletes it with its `target/`. When `target/` is a btrfs subvolume the seed is an instant snapshot, otherwise a reflink copy taking minutes; `prepare` turns this checkout's `target/` into a subvolume once (run it while no build uses `target/`) |
| `cargo xtask push` | Push the current branch with `git push` and open its pull request into `main` as a draft when it has none; no checks and no review run. It refuses `main` and a detached `HEAD`, and replaces a rebased branch's remote copy with `--force-with-lease` bound to the copy it saw. A person marks the pull request ready and merges it |

Cargo runs offline here (`.cargo/config.toml`).

## Output

`doc`, `lock` and the checks print one line per step. The output of the
commands they run goes to a log under `target/xtask/logs/` (kept three
days); a failure prints that log's diagnostics, at most 60 lines, and its
path. `--verbose` streams everything instead.

## Firmware lockfiles

Firmware builds never modify a committed `Cargo.lock`. Each build copies the
workspace catalog into its own cache and resolves through Cargo's
`resolver.lockfile-path` (Cargo 1.97+), so a `[patch]` or local override
writes only that copy, which is archived as the build's effective lockfile.
Builds of different image classes run concurrently. An overlapping build of
the same output fails on that copy's lease; the artifact lease separately
protects the selected cache and output snapshot. Successful bundles remain
available for inspection, while failed partial bundles are removed. Flashing
uses the completed bundle after releasing the build lease, so a later build
cannot replace the selected image. The serial-device lease independently
protects the complete hardware write transaction.

## Contract diagnostics

`cargo xtask check ui` compiles every case in `tests/ui.toml` twice, with the
same dependency features and target: its valid use, then the misuse selected
by `ui_reject`. A `// ui-error: E0123` annotation marks the expected error's
source line. The check reads rustc JSON diagnostics and rejects a different
error code, a different location, extra errors, a successful misuse, or a
broken positive twin. Stable rustdoc's `compile_fail,E0123` does not check
the code; those examples remain documentation, not this evidence.

For a rustc diagnostic without a code, `// ui-error: null exact message`
requires both `code: null` and that complete message at the marked line.
Repeat the annotation on that line for each expected diagnostic; the check
does not accept fewer or more errors at the same boundary.

Cargo supplies exact library artifact paths for each declared target and
feature set; the runner never selects an rlib by globbing a shared cache.
Both host and RV32 use the pinned stable toolchain. New cases belong beside
the contract they exercise and have an explicit entry in the catalog.
