# ESP32-S31 boot binding

The chip's cold-start composition supplies LP-fast, brownout and TRNG profiles
and `S31: staged_bootstrap::startup::BootProfile`. It binds the root's owned HAL
parts and existing CPU, PSRAM, cache and flash profiles to the family runner.
Its Composition role permits that platform wiring; startup order stays in
`oer-espressif-staged-bootstrap`, and block algorithms in `oer-espressif-boot`.
Neither HAL nor chip knowledge depends on this binding. LP-fast comes from
tracked chip/profile facts, generated with the register model's named source.

The bootstrap image declares its descriptor, payload and tuning resources and
calls the family runner. The binding delegates leaf operations to the same
HAL atoms and family drivers, preserving observations, waits and deadline.
Runtime adoption remains in the staged runtime and HAL runtime root.
