# Repository model

`oer-repo` is the one model of the repository's own structure. Every tool
that reasons about the tree as a whole — `oer-tidy`, `oer-xtask`, the
HIL source snapshot and the vendor evidence
harness — reads it here instead of listing files, parsing manifests or
asking `cargo metadata` itself. It reads text only: no Cargo resolution, no
compilation, so loading the whole tree takes a fraction of a second.

| Module | Answers |
| --- | --- |
| [`files`](src/files.rs) | The file inventory: a checkout's tracked and untracked-but-not-ignored files (`git ls-files`), without any `target`, `_oracles` or `.git` component; tracked files apart |
| [`index`](src/index.rs) | The index view of a checkout, `IndexSnapshot::read(root)`: its `HEAD` commit, whether it is dirty, every index path (a tracked file deleted from the worktree and a symlink included) and its untracked, unignored files, nothing skipped; what the HIL source snapshot archives |
| [`manifest`](src/manifest.rs) | Every `Cargo.toml`: packages, crate roots, library and README, features, dependencies (kind, target table, rename, path, features, optional, inherited from `[workspace.dependencies]`), workspace declarations |
| [`workspaces`](src/workspaces.rs) | The workspaces Cargo finds, explicit and implicit, and the one each package belongs to |
| [`chips`](src/chips.rs) | Every `platform/<chip>/chip.toml`, through `oer-chip-profile` |
| [`classification`](src/classification.rs) | Every key of `[package.metadata.open-radio]`, typed: layer (and the scope it implies), platform, chip, family, evidence and HIL roles, the host layer (`host-layer`, required of every host development package), supported feature profiles, test feature sets, inputs |
| [`policy`](src/policy.rs) | Which package may depend on which: layers, platforms, executor and time-driver bindings, evidence and HIL role edges ([layer dependencies](../../docs/architecture.md#layer-dependencies)), and host layers ([host layers](../../docs/architecture.md#host-layers): entry, orchestration, execution, verification, stand, build, foundation; a host package depends only on its own layer and those below), plus `COMPONENTS`, the package-level table of the [shared RV32 layer](../riscv/README.md#dependency-table) and Blobray |
| [`closure`](src/closure.rs) | The path packages a package reaches, by dependency kind, target triple (`cfg(…)` evaluated, failing on a cfg it cannot evaluate) and features (resolved from the roots as Cargo unifies them, or every optional dependency); the packages that reach a set |
| [`lock`](src/lock.rs) | The entries of a `Cargo.lock` |

`Model::load(&Repo::load(root)?)` reads everything once.
`Model::owner(path)` names the package a repository path belongs to across
every workspace, `Model::package(name)` finds a package by its unique name,
`Model::classification(package)` returns its typed classification, and
`Model::closure(roots, edges, target, features)` and
`Model::dependents(candidates, selected)` walk the path-dependency graph.

HIL roles: `observation` code shapes what a run observes and reaches no
stand code; `orchestration` code drives runs (the runner, its families,
images, the stand library) and may use stand operation; `operation` code
only operates the stand (boards, flashing, the stand file) and never shapes
a passed observation.

Host packages declare typed `host-app` ownership and `host-boundary`
(`application`, `library`, `format`) alongside their host layer. The path
dependency policy checks application boundaries independently of layers;
[`architecture`](../../docs/architecture.md#host-applications) defines the
shared consumers. Every host-layer package declares both keys.
