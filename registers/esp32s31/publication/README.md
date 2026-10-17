# Source-only register publication

The [native manifest](registers.toml) selects reviewed hardware geometry, sparse
assertions, applicability, memory/ownership policy, the restricted PAC API pack,
lints and the source evidence catalogs. It does not select a vendor investigation or
binary model.

```console
cargo registers validate --manifest registers/esp32s31/publication/registers.toml
cargo registers generate --manifest registers/esp32s31/publication/registers.toml --check
```

The second command checks SVD, raw PAC, restricted PAC API and binding index
together. Omit `--check` to publish an intentional reviewed change. Missing or
invalid selected sources fail validation. Evidence catalog references document
provenance; this source-only operation does not open/authenticate vendor binaries
or qualify hardware behavior. See the [tool contract](../../../tools/registers/README.md).
