# ESP32-S31 raw PAC

This crate is restricted implementation detail for
`oer-esp32s31-pac`. It intentionally exposes the low-level
svd2rust API required to implement reviewed transactions.

`src/lib.rs` is the generated backend and the package's only source file.
Handwritten code that needs raw register access, such as the IEEE 802.15.4
task/interrupt split and its validation transactions, lives in the closed
parent `pac` crate.

Do not depend on it from HAL, driver or application crates.
Physical pointers, `steal` and raw register writers are not product APIs. The
workspace architecture test permits this dependency only from the adjacent
closed `pac` crate.

Regenerate it from the reviewed source-only publication manifest:

```console
cargo registers generate \
  --manifest registers/esp32s31/publication/registers.toml
```

Use `--check` to validate reproducibility without overwriting the output.
The [semantic PAC map](../pac/README.md) identifies the register model, API policy
and the other published artifacts.
