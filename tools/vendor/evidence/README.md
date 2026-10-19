# oer-vendor-evidence

The native vendor-comparison evidence index of a chip
(`target/verification/<chip>/evidence`, derived data that nothing tracks):
one JSON shard per scenario, so each scenario run rewrites only its own file.
Producers write it, `cargo vendor-check evidence` computes it for the
checkout, and the qualification evaluator reads it.
The shared data contract and store live in
[`oer-vendor-evidence`](SHARD.md), a root-workspace format
package. This verification workspace library owns comparison policy and
producer orchestration. Qualification reads the format directly.

| Module | Owns |
| --- | --- |
| `index` (re-exported at the root) | The shard format `Index` (schema, producer `command`, claims, coverage, observation, state, untriaged locations), `validate`, `is_current` against the recorded source digests and coverage decisions, `digest_source`, `Evidence` (a whole derived directory, where a stale shard, one binding a source gone since and one of another format schema in `other_schema` are staleness rather than errors) and `untriaged` (locations no scenario covers) |
| `store` | The one reader and writer of shard files: `path`, `write`, `read`, `names` and `shards` |
| `policy` | The verdict source policy: `closure` (the path packages a shard may record, report packages apart), `report_packages`, `check_verdict_sources` and `reject_report_sources` for the shards of an index |
| `diff` | What two versions of one shard say differently: claims line by line, recorded sources counted |
| `baseline` (feature `producers`) | A change's index against the one `main` published: `claims` (each claimed vendor root with its reach, read from the fields every format names), `losses` (roots lost or whose comparisons reach fewer blocks or directions) and `Accepted`, the chip's reviewed `decisions/accepted-losses.toml` (`cargo vendor-check evidence --compare`) |
| `producer` | The `Producer` contract and the host stands: `host_stand::stands` (a package below `verification/<chip>/host/` owns the shard `<directory>-host`) and `host_stand::shard`, the shard a stand writes from its own `shard` command |
| `run` (feature `producers`) | The producers' orchestration: `probes` (each chip's Rust comparison probe images, built and their catalogs validated: `cargo vendor-check probes`), `scenario` (the typed Blobray scenarios built in the Blobray workspace and run: `cargo vendor-check scenario`), `regenerate` (which producer computes which shard of the derived index, whole or named, and the untriaged listing: `cargo vendor-check evidence`) and `phase` (the `phase <name>: <seconds> s` timing lines) |

The producers are the Blobray scenario engine
([`harness/scenarios`](../scenario), `command = "vendor-scenario"`)
and the host stands ([`esp32s31/host/i`](../ieee802154-esp32s31/README.md),
`command = "vendor-host"`). Which of them reruns which shard is decided by
`run::regenerate`, which `cargo vendor-check evidence` (xtask) calls.

Without `producers` this library exposes comparison policy and the index
API. `producers` adds toolchain, chip profiles and the probe orchestration.
Qualification reads only the shared format.

```console
cargo test -p oer-vendor-evidence --features producers
```
