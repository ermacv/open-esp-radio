# Tidy

`oer-tidy` prints the repository model of [`oer-repo`](../repo/README.md)
and fills the Cargo cache, with no Cargo resolution and no compilation.

```console
cargo tidy workspaces --json
cargo tidy chips --json
cargo tidy fetch
```

`workspaces` prints every Cargo workspace and `chips` every chip profile
(`platform/<chip>/chip.toml`) of that model, one per line or as a JSON
array. `fetch` downloads what every workspace's lock names and the Cargo
cache lacks, online only for a workspace that misses something; it needs
only this small package built, so it works after a pull that changed what
xtask itself depends on.

`oer-tidy` has its own workspace and lock file containing only registry
packages and host foundations. A changed firmware Git pin cannot prevent
its offline startup. `cargo tidy` runs `cargo run --quiet --manifest-path
tools/tidy/Cargo.toml -p oer-tidy --`; `--root DIR` reads another checkout.
Its host build outputs live in `tools/tidy/target/`. If its own registry
dependencies are absent from Cargo's offline cache, bootstrap with
`cargo --config net.offline=false tidy fetch`. A moved firmware Git pin alone
does not require this override.
