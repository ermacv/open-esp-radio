# oer-pins-artifacts

The pinned vendor artifacts of a chip. `chips/<chip>/vendor/artifacts.toml`
is the only pin: git sources at a revision and release assets, each
artifact with its SHA-256, and firmware outputs, pinned by the tracked
catalog image whose recipe builds them and by no digest. This crate is its only reader
(`Manifest::parse`, `Manifest::load`, typed `Source` and `Artifact`,
`Manifest::location`, `artifacts::fetched(root, chip, id)` for one verified artifact
such as the ROM ELF the firmware stack gate reads). It resolves every
artifact in the host-wide store (`$OER_VENDOR_CACHE`, default `open-esp-radio/vendor` in the
user's XDG cache directory, through `oer-os-durable`, linked from each
checkout's `target/vendor`), and fetches and verifies what is missing.

It reads chip profiles and facts through `oer-chip-profile` and
`oer-espressif-facts`, and uses `serde`, `toml` and the host process and
durable-file libraries. The host IEEE 802.15.4 stand's build script and the
typed vendor scenarios read the pins through it too.

The `cargo pins fetch` program is in `tools/pins/cli`. Vendor checks and
the [image pipeline](../../image/build/README.md) also read the pins through
this library; `oer-image-esp-idf` manages the pinned ESP-IDF checkout and its
submodules.
