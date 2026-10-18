# ESP32-S31 HIL target

- [System and PHY watchdogs](watchdogs.md)
- [Stack measurements](memory.md)
- [Host setup and fixture installation](../../host/README.md)
- [Wire and evidence contracts](../../protocol/README.md)

This workspace selects the shared board boot and memory profile and owns the
Embassy executor, the USB Serial/JTAG transport and the HIL workloads of the
radio-free system images. Effective dependency locks are archived beside each
image. The radio images and their workloads went with the orchestration in
; tag `legacy-radio-2026-10-16` holds them.

## Application boundaries

The agent binary is the HIL application of the shared stage-two boot
contract. Each image class selects one base feature: `system-watchdog`,
`system-panic-reset` or `boot-smoke` (`usb-jtag-off` adds to
`system-watchdog`). Board initialization, relocation and interrupt-stack
mechanics belong to the shared platform; an application image would use the
same mechanism through `oer-image`. A hardware scenario verdict remains a
separate HIL responsibility.

## Protocol families per image

Each image enables only the HIL protocol families it serves
([radio families](../../protocol/README.md#radio-families)): the radio-free
`system-watchdog`, `diagnostic-usb-jtag-off` and `system-panic-reset` images
read only the shared core, `system` module included.

## Panic and exception records

The agent's panic hook, which the platform's panic entry calls through its
`panic-hook` feature, runs in every image but `system-panic-reset` (the
agent's `panic-hook` feature, which each image's base feature enables); that
image runs the product panic entry alone, so the `system-panic-reset`
scenario exercises the product's record-and-reset path. The hook and the
exception entry format nothing: they record the post-mortem fault and the
hart's trap registers, the vector it took and the sources pending and routed
to it (`agent/src/fatal.rs`) in RTC fast memory and reset, and the next boot
prints `OPEN_RADIO_HIL runtime=PANIC boot=previous` or `runtime=EXCEPTION
boot=previous` with that state before anything else. A panic message with
arguments is not kept, only a static one in task context. An
interrupt-context panic omits location and message because compiler metadata
may be in unavailable PSRAM; the hook receives the platform's safe
`PanicDetails`. The hook's code and diagnostic stage stay in SRAM, its
records stay in RTC memory, and its fault CRC uses no cached lookup table.

## Local dependency overrides

A git-pinned dependency of the firmware can be replaced by a local checkout
for one build or run. The variable names the checkout's root:

| Variable | Checkout | Packages patched |
| --- | --- | --- |
| `ESP_HAL_ROOT` | esp-hal | `esp-hal`, `esp-sync`, `esp-bootloader-esp-idf` |

The runtime build patches them in and resolves a private copy of its lock
file; the bootstrap takes only the esp-hal override, the one dependency it has.
A run's source snapshot archives each override checkout as its own source
role, so an untracked file there is named as `--source-include esp-hal:<path>`.
An override build binds evidence only when the checkout's commit is the pinned
revision in `hil/targets/esp32s31/Cargo.toml`; otherwise it is an experiment.

To find the dependency commit that changed a scenario's outcome, bisect in the
dependency's clone and build this checkout against each step. A build failure
skips the step (exit 125) instead of marking it bad:

```console
git clone https://github.com/ermacv/esp-hal.git ~/dev/esp-hal
cd ~/dev/esp-hal
git bisect start <bad-rev> <good-rev>
git bisect run sh -c 'cd ~/dev/<checkout> &&
  ESP_HAL_ROOT=~/dev/esp-hal cargo hil image build system-watchdog || exit 125;
  ESP_HAL_ROOT=~/dev/esp-hal cargo hil run <scenario>'
git bisect reset
```

Each step takes its own lease.
