# ESP32-S31 bootstrap

The first-stage image: one reset root, bounded boot sequences, then stage two.

`src/entry.rs` declares the descriptor, payload and tuning resources and calls
the family runner, which owns the startup order
([`oer-chips-family-platform-bootstrap`](../../../family/platform/bootstrap/README.md));
block algorithms live in `oer-chips-family-boot`. The `profile` module forbids
unsafe code and supplies the LP-fast, brownout and TRNG profiles and
`S31: staged_bootstrap::startup::BootProfile`, binding the root's owned HAL
parts and the existing CPU, PSRAM, cache and flash profiles to that runner.
LP-fast comes from the tracked chip facts, generated with the register model's
named source. Neither the HAL nor chip knowledge depends on this crate.

Leaf operations delegate to the HAL atoms and family drivers, preserving
observations, waits and deadline. Runtime adoption remains in the staged
runtime and the HAL's runtime root.
