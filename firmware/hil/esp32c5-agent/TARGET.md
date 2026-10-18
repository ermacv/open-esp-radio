# ESP32-C5 HIL target

This workspace builds the esp32c5's HIL runtime, `oer-esp32c5-hil-agent`,
for `riscv32imac-unknown-none-elf`, as the stage two of the
[staged boot](../../../platform/esp32c5/README.md): the platform bootstrap
copies it into PSRAM and enters it. It runs the scheduler-free Embassy
executor of `oer-espressif-executor-embassy` on the image's interrupt table.

`boot-smoke` reports `OPEN_RADIO_HIL hal=INIT`
after esp-hal initialization, `OPEN_RADIO_HIL embassy=START` once the
executor runs, and `OPEN_RADIO_HIL boot-smoke=PASS timer=PASS` after one
timer wake, on the USB Serial/JTAG console. It does not speak the HIL wire
protocol. The runner builds it for the `boot-smoke` scenario through the
image pipeline and flashes the bundle at the chip profile's `[flash]` offsets.
The write leaves the ROM in download mode, and an RTS reset out of it starts
the image with its USB console silent (the host reads EOF); the runner starts
it with a power-on reset instead, cycling the board's hub port and waiting
for its port to return, so the board's stand file entry needs `power` in its
`reset` ladder:

```console
cargo hil run --chip esp32c5 boot-smoke
```

`system-watchdog` serves the HIL wire protocol on the USB Serial/JTAG
console and serves `system::WatchdogTest`,
and nothing radio. Its SoC deadline watchdog is the production service of
[`oer-esp32c5-soc-esp-hal`](../../../crates/adapters/esp-hal/esp32c5/soc/)
on TIMG1's main watchdog; its boot evidence maps the chip's reset reasons
(`CoreMwdt1` is `MainWatchdog1`) and reads the reset-retained post-mortem and the
platform panic record in LP RAM. The runner classifies a flashed esp32c5 image from its
image keys, and `system-watchdog` drives every watchdog mode on it:

```console
cargo hil run --chip esp32c5 system-watchdog
```

A run of either replaces the board's peer firmware; a run that needs the peer
reflashes its catalog image first.
