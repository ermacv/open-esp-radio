# ESP32-C5 open questions

Work that is known and decided but not done yet, and questions still open.
An item leaves this file in the change that settles it.

## Open

- The C5 HIL agent does not build: its stack policy,
  `firmware/hil/esp32c5/stack.toml`, does not exist, because the C5's stack
  bounds (the task stack, the interrupt stack and the bootstrap's) are not
  measured yet. They are measured when the C5 work starts; until then
  `cargo hil image build --chip esp32c5` fails on the missing policy.
