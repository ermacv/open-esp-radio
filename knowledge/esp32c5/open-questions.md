# ESP32-C5 open questions

Questions still open. An item leaves this file in the change that settles it.

## Open

- The C5 HIL agent does not build: its stack policy,
  `firmware/hil/esp32c5/stack.toml`, does not exist, because the C5's stack
  bounds (the task stack, the interrupt stack and the bootstrap's) are not
  measured; `cargo hil image build --chip esp32c5` fails on the missing
  policy.
