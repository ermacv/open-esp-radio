# oer-vendor-artifacts

The pinned vendor artifacts of a chip. `verification/<chip>/artifacts.toml`
is the only pin: git sources at a revision and release assets, each
artifact with its SHA-256, and firmware outputs, pinned by the tracked
catalog image whose recipe builds them and by no digest. This crate is its only reader
(`Manifest::parse`, `Manifest::load`, typed `Source` and `Artifact`,
`Manifest::location`, `fetched(root, chip, id)` for one verified artifact
such as the ROM ELF the firmware stack gate reads). It resolves every
artifact in the host-wide store (`$OER_VENDOR_CACHE`, default `open-esp-radio/vendor` in the
user's XDG cache directory, through `oer-durable`, linked from each
checkout's `target/vendor`), and fetches and verifies what is missing.

It depends only on `toml`, `serde` and the foundation crates, so the host
IEEE 802.15.4 stand's build script and the Blobray workspace's vendor
scenarios read the pins through it too.

`cargo verification fetch` and the other `cargo verification` commands
read the pins through it, and so does the HIL
stand's pinned ESP-IDF build in [`oer-hil-cli`](../../hil/host/cli/README.md),
which checks out the pinned IDF and its pinned submodules.
