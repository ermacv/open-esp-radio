# Bootstrap observation

This image uses the normal production bootstrap, module memory profile and
strict PSRAM density check. Its stage-two entry only installs the early
console and acknowledges handoff; it starts no radio, task or interrupt.

Build the probe with `cargo fw probe oer-firmware-probes-esp32s31-bootstrap`.
For one short boot under the selected board's lease:

```console
cargo stand --owner stand lease --board s31-c --air none -- \
  cargo fw run ../probes/esp32s31-bootstrap --board s31-c --timeout 5s
```

A successful handoff prints `OER-RESULT: pass psram-bootstrap`. An earlier
bootstrap failure prints its typed reason and halts. The runner then times
out; the failure line, rather than that timeout, identifies the boot outcome.
The `found` field of a density mismatch is the decoded size in bytes, not
an independent measurement of the fitted device's capacity.
